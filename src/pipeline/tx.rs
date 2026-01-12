use std::{
    cmp,
    ops::{Deref, DerefMut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_mutex::{Mutex, MutexGuard};
use thiserror::Error;

use super::{
    LOCAL_EPOCH, Status,
    ringbuf::{
        Pull, RingBufferReaderMPSC, RingBufferReaderSPMC, RingBufferWriterMPSC, RingBufferWriterSPMC, ringbuffer_mpsc,
        ringbuffer_spmc,
    },
    status,
};
use crate::{
    buffers::{
        BoxBuf, Bytes,
        reader::Reader,
        writer::{BacktrackableWriter, DidntWrite, HasWriter, Writer},
    },
    codec::{LCodec, ThuboCodec, WCodec},
    protocol::{
        BatchSize, CongestionControl, FragmentHeader, FragmentKind, FrameHeader, Message, MessageBody, MessageHeader,
        Priority, QoS, SeqNum,
    },
    sync::{Notifier, Waiter, event},
};

// AtomicBackoff
struct AtomicBackoff(AtomicU64);

impl AtomicBackoff {
    const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    fn clear(&self) {
        self.0.store(0, Ordering::Release);
    }

    fn start(&self, first_write: Duration) {
        self.0
            .store((first_write.as_micros() as u64) << BatchSize::BITS, Ordering::Release);
    }

    fn update(&self, bytes: BatchSize) {
        self.0.fetch_or(bytes as u64, Ordering::AcqRel);
    }

    fn load(&self) -> (BatchSize, Duration) {
        let value = self.0.load(Ordering::Acquire);
        let bytes = value as BatchSize;
        let microseconds = value >> BatchSize::BITS;
        let first_write = Duration::from_micros(microseconds);
        (bytes, first_write)
    }
}

// WriteBatch
#[derive(Debug)]
pub(crate) struct WriteBatchHeap {
    pub(crate) frame: BoxBuf,
    pub(crate) fragment: Bytes,
    pub(crate) qos: QoS,
    pub(crate) drop: Option<Arc<AtomicBool>>,
}

#[derive(Debug)]
pub(crate) struct WriteBatch(Box<WriteBatchHeap>);

impl Deref for WriteBatch {
    type Target = WriteBatchHeap;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for WriteBatch {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl WriteBatch {
    const BATCH_LEN: usize = BatchSize::MAX.to_le_bytes().len();

    fn alloc(capacity: BatchSize) -> Self {
        let inner = WriteBatchHeap {
            frame: BoxBuf::with_capacity(Self::BATCH_LEN + capacity as usize),
            fragment: Bytes::new(),
            qos: QoS::DEFAULT,
            drop: None,
        };
        let mut s = WriteBatch(Box::new(inner));
        s.clear(); // Reserve space for frame length
        s
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn len(&self) -> BatchSize {
        let len = self.frame.len() - Self::BATCH_LEN + self.fragment.len();
        len as BatchSize
    }

    fn write_len(&mut self) {
        let len = self.len();
        let mut writer = self.frame.as_mut_slice().writer();
        writer.write_exact(&len.to_le_bytes()).unwrap();
    }

    fn clear(&mut self) {
        self.frame.clear();
        self.fragment.clear();
        self.drop = None;

        // Reserve space for frame length
        let mut writer = self.frame.writer();
        writer.write_exact(&(0 as BatchSize).to_le_bytes()).unwrap();
    }
}

/// Errors that can occur when sending a message through the transmission
/// pipeline.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SendError {
    /// Message was dropped due to congestion control policy.
    ///
    /// This error occurs when:
    /// 1. The message uses [`CongestionControl::Drop`] mode (best-effort delivery)
    /// 2. The priority level for this message is currently congested
    ///
    /// When congestion is detected, messages configured with drop semantics are
    /// immediately discarded to prevent backpressure and maintain low latency.
    /// The sender marks the priority level as congested after this error
    /// occurs.
    ///
    /// Messages using [`CongestionControl::Block`] mode will never receive this
    /// error - they will wait for available capacity instead.
    #[error("Message was dropped due to congestion control policy")]
    Dropped,

    /// Timed out accessing the queue or waiting for an available batch to be refilled.
    ///
    /// This error indicates that the sender could not acquire a write batch
    /// within the configured timeout period. This typically means:
    /// - Another thread is sending a large message and keeping the lock on the queue
    /// - Network transmission is slower than message production rate: the refill stage is empty
    ///
    /// After this timeout, the priority level is marked as congested.
    /// Subsequent messages with [`CongestionControl::Drop`] will be
    /// immediately dropped without waiting for a batch.
    ///
    /// This is a backpressure signal indicating the application should reduce
    /// its send rate for this priority level.
    #[error("Timed out accessing the queue or waiting for an available batch")]
    Timeout,

    /// Message encoding failed during serialization.
    ///
    /// This error occurs when the codec fails to serialize a message into
    /// the batch buffer. This typically indicates a batch size too small.
    #[error("Message encoding failed during serialization")]
    EncodingFailed,

    /// The transmission pipeline has been closed.
    ///
    /// This error occurs when the receiving end of the pipeline (the consumer)
    /// has been dropped or explicitly closed, making it impossible to send
    /// further messages. Once closed, all subsequent send attempts will fail
    /// with this error.
    ///
    /// Common causes:
    /// - The network connection was closed
    /// - The receiver task was terminated
    /// - The pipeline is shutting down
    ///
    /// This is typically a terminal error - the pipeline cannot be reopened
    /// and a new connection must be established to continue sending messages.
    #[error("Pipeline closed - receiver has been dropped")]
    Closed,

    /// An internal error occurred during message processing.
    ///
    /// This error indicates a bug in the pipeline implementation and should
    /// not occur under normal circumstances. If you encounter this error,
    /// please report it as a bug.
    #[error("Message was dropped due to an internal error. This is a bug.")]
    Internal,
}

struct SeqNumPool {
    /// Sequence number generator for reliable (Block) messages.
    reliable: SeqNum,
    /// Sequence number generator for best-effort (Drop) messages.
    best_effort: SeqNum,
    /// Fragment ID generator for fragmenting oversized messages.
    frag_id: SeqNum,
}

impl SeqNumPool {
    fn new() -> Self {
        Self {
            reliable: SeqNum::new(0),
            best_effort: SeqNum::new(0),
            frag_id: SeqNum::new(0),
        }
    }
}

struct StageInPrio {
    // State
    seq_num: SeqNumPool,
    // Out
    ringbuf_inout_w: RingBufferWriterMPSC<WriteBatch>,
    // Refill
    stage_refill: StageRefill,
}

impl StageInPrio {
    fn flush(&mut self) -> bool {
        match self.ringbuf_inout_w.peek() {
            Some(peek) => {
                peek.commit();
                true
            }
            None => false,
        }
    }

    async fn push(
        &mut self,
        msg: &Bytes,
        config: &PushConfig,
        start: Duration,
        backoff: &AtomicBackoff,
        notify: impl Fn() -> Result<(), SendError>,
        flush: impl Fn() -> bool,
    ) -> Result<bool, SendError> {
        // Extract QoS parameters from config
        let congestion_control = config.qos.congestion_control();
        let express = config.qos.express();

        // Drop flags for fragments
        let mut drop_flag: Option<Arc<AtomicBool>> = None;

        /// Macro to acquire a batch for serialization.
        ///
        /// This macro attempts to:
        /// 1. Take the existing batch from the guard if available
        /// 2. Otherwise, pull a new batch from the refill stage
        /// 3. If no batch is available, wait with timeout and retry
        ///
        /// Returns a `Batch` batch or propagates `SendError::Timeout`.
        macro_rules! batch_or_return {
            () => {
                loop {
                    if let Some(batch) = self.ringbuf_inout_w.peek() {
                        break batch;
                    }

                    if let Some(mut batch) = self.stage_refill.pull() {
                        // Initialize batch state
                        backoff.start(LOCAL_EPOCH.elapsed());
                        batch.qos = config.qos;
                        batch.drop = drop_flag.clone();
                        break self
                            .ringbuf_inout_w
                            .book(batch)
                            .map_err(|_| SendError::Internal)?;
                    }

                    // No batch available, wait with timeout
                    let df = drop_flag.as_ref();

                    macro_rules! set_congested {
                        () => {{
                            df.inspect(|df| df.store(true, Ordering::Relaxed));
                        }};
                    }

                    let timeout = LOCAL_EPOCH
                        .elapsed()
                        .checked_sub(start)
                        .and_then(|elapsed| config.timeout.checked_sub(elapsed))
                        .ok_or(SendError::Timeout)
                        .inspect_err(|_| set_congested!())?
                        .min(config.timeout_batch);
                    tokio::time::timeout(timeout, self.stage_refill.can_refill.wait())
                        .await
                        .map_err(|_| SendError::Timeout)
                        .inspect_err(|_| set_congested!())?
                        .map_err(|_| SendError::Dropped)
                        .inspect_err(|_| set_congested!())?;
                }
            };
        }

        // Acquire the current serialization batch (or wait for one to become available)
        let mut batch = batch_or_return!();

        macro_rules! move_batch {
            () => {
                backoff.clear();
                batch.commit();
                notify()?;
            };
        }

        macro_rules! return_notify {
            () => {{
                if express || flush() {
                    move_batch!();
                    return Ok(false);
                } else {
                    backoff.update(batch.len());
                    return Ok(true);
                }
            }};
        }

        // Initialize the codec for serialization
        let codec = ThuboCodec::new();

        // === STEP 1: Append to existing frame (optimization) ===
        {
            // Optimization: If the current batch has a frame with matching congestion control,
            // we can append this message directly without creating a new frame header. This
            // reduces overhead and improves batching efficiency.
            if !batch.is_empty() {
                if batch.qos.congestion_control() == congestion_control {
                    // Congestion control matches - try to append message to existing frame
                    let mut writer = batch.frame.writer();
                    let mark = writer.mark();
                    let mut encode = || codec.write(&mut writer, msg);
                    match encode() {
                        Ok(()) => return_notify!(),
                        Err(_) => {
                            // Batch is full, rewind the write attempt
                            writer.rewind(mark);
                        }
                    }
                }

                // Batch is full or congestion control differs - flush it and get a fresh batch
                move_batch!();
                batch = batch_or_return!();
            }
        }

        // === STEP 2: Create new frame on fresh batch ===
        //
        // Select the appropriate sequence number generator based on congestion control.
        // Best-effort and reliable messages use independent sequence number counters to
        // avoid head-of-line blocking between different QoS levels.
        let seq_num_gen = match congestion_control {
            CongestionControl::Drop => &mut self.seq_num.best_effort,
            CongestionControl::Block => &mut self.seq_num.reliable,
        };

        // Generate the next sequence number and create frame header
        let seq_num = seq_num_gen.next();
        let mut thubo = Message::new(
            MessageHeader::new(config.qos, seq_num),
            MessageBody::Frame(FrameHeader::new()),
        );

        // Try to serialize frame header + message on the fresh batch
        {
            let mut writer = batch.frame.writer();
            let mark = writer.mark();
            let mut encode = || {
                codec.write(&mut writer, &thubo)?;
                codec.write(&mut writer, msg)?;
                seq_num_gen.set(thubo.header.seq_num);
                Ok::<(), DidntWrite>(())
            };
            match encode() {
                Ok(()) => return_notify!(),
                Err(_) => {
                    // Even a fresh batch can't hold this message - it must be fragmented
                    writer.rewind(mark);
                }
            }
        }

        // === STEP 3: Message fragmentation ===
        //
        // If we've reached this point, the message is too large to fit in a single batch
        // even when the batch is empty. We must fragment it across multiple batches.
        //
        // Fragmentation process:
        // 1. Generate a unique fragment ID to track all fragments of this message
        // 2. Create a First fragment with the total message length
        // 3. Stream message data into fragments, creating More fragments as needed
        // 4. Mark the final fragment as Last
        //
        // Each fragment is sent immediately to maintain ordering and avoid buffering
        // large messages in memory.
        {
            drop_flag = Some(Arc::new(AtomicBool::new(false)));
            batch.drop = drop_flag.clone();

            // Generate a unique fragment ID to track all fragments belonging to this message
            let frag_id = self.seq_num.frag_id.increment();

            // Create the First fragment header, which includes the total message length
            // so the receiver can pre-allocate the reassembly buffer
            thubo.body = MessageBody::Fragment(FragmentHeader::new(FragmentKind::First, frag_id));

            let mut writer: &mut BoxBuf = batch.frame.writer();
            codec
                .write(&mut writer, &thubo)
                .map_err(|_| SendError::EncodingFailed)?;
            codec
                .write(&mut writer, msg.len())
                .map_err(|_| SendError::EncodingFailed)?;

            // Create a reader to stream message data into fragments
            let mut reader = msg.reader();

            // Fill the first fragment with as much data as possible
            reader
                .read_chunks(writer.remaining(), |chunk| {
                    batch.fragment.push(chunk);
                })
                .map_err(|_| SendError::EncodingFailed)?;

            // Commit the sequence number and send the first fragment immediately
            seq_num_gen.set(thubo.header.seq_num);
            move_batch!();

            // Generate sequence number for the next fragment
            thubo.header.seq_num = seq_num_gen.next();

            // Continue fragmenting until all message data has been sent
            while reader.can_read() {
                // Acquire a batch for this fragment
                batch = batch_or_return!();

                let mut writer: &mut BoxBuf = batch.frame.writer();

                // Start with a More fragment (may be upgraded to Last below)
                thubo.body = MessageBody::Fragment(FragmentHeader::new(FragmentKind::More, frag_id));

                // Determine if this will be the last fragment by comparing available space
                // with remaining data
                let w_capacity = writer.remaining() - codec.w_len(&thubo);
                let r_capacity = reader.remaining();
                if w_capacity >= r_capacity {
                    // All remaining data fits - this is the Last fragment
                    thubo.body = MessageBody::Fragment(FragmentHeader::new(FragmentKind::Last, frag_id));
                }

                // Write the fragment header (either More or Last)
                codec
                    .write(&mut writer, &thubo)
                    .map_err(|_| SendError::EncodingFailed)?;

                // Stream message data into this fragment
                reader
                    .read_chunks(r_capacity.min(w_capacity), |chunk| {
                        batch.fragment.push(chunk);
                    })
                    .map_err(|_| SendError::Internal)?;

                // Commit the sequence number
                seq_num_gen.set(thubo.header.seq_num);

                // Send this fragment immediately to maintain ordering
                move_batch!();

                // Generate sequence number for the next fragment (if any)
                thubo.header.seq_num = seq_num_gen.next();
            }

            Ok(false)
        }
    }
}

/// Configuration for sending messages through the pipeline.
#[derive(Clone, Copy)]
pub(crate) struct PushConfig {
    /// Quality of Service settings (priority, reliability, express mode).
    pub(crate) qos: QoS,
    /// Timeout for pushing a whole message.
    pub(crate) timeout: Duration,
    /// Timeout for acquiring a new batch.
    pub(crate) timeout_batch: Duration,
}

struct StageIn {
    priorities: [Mutex<StageInPrio>; Priority::NUM],
    backoff: [AtomicBackoff; Priority::NUM],
    can_pull: Notifier,
    status: Status,
}

impl StageIn {
    #[inline]
    pub(crate) async fn push(&self, msg: &Bytes, config: &PushConfig) -> Result<(), SendError> {
        let start = LOCAL_EPOCH.elapsed();
        let p = config.qos.priority();

        // Lock the queue: try first to try_lock and then a lock with timeout (which is slow)
        let mut queue = match self.priorities[p].try_lock() {
            Some(queue) => queue,
            None => tokio::time::timeout(config.timeout, self.priorities[p].lock())
                .await
                .map_err(|_| SendError::Timeout)?,
        };

        let notify = || {
            if !self.status.any(status::BACKOFF) {
                self.can_pull.notify().map_err(|_| SendError::Closed)?;
            }
            Ok::<(), SendError>(())
        };
        let flush = || {
            let f = status::flush(p);
            status::any(self.status.unset(f), f)
        };

        let to_notify = queue.push(msg, config, start, &self.backoff[p], notify, flush).await?;
        drop(queue);

        if to_notify {
            notify()?;
        }

        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct PipelineTxIn {
    /// Array of input stages, one per priority level (mutex-protected for
    /// concurrent access).
    stage_in: Arc<StageIn>,
}

impl PipelineTxIn {
    pub(crate) fn status(&self) -> &Status {
        &self.stage_in.status
    }

    #[inline]
    pub(crate) async fn push(&self, msg: &Bytes, config: &PushConfig) -> Result<(), SendError> {
        let is_drop = config.qos.congestion_control() == CongestionControl::Drop;
        let p = config.qos.priority();
        let congested = status::congested(p);

        // Fast path: drop immediately if priority is congested and using drop mode
        if is_drop && self.stage_in.status.any(congested) {
            return Err(SendError::Dropped);
        }

        self.stage_in.push(msg, config).await.inspect_err(|e| match e {
            SendError::EncodingFailed | SendError::Closed | SendError::Internal => {}
            SendError::Dropped | SendError::Timeout => {
                self.stage_in.status.set(congested);
            }
        })
    }

    pub(crate) async fn disable(&self) {
        self.status().set(status::DISABLED);

        // Acquire all locks in order to avoid deadlocks
        // (same order as drain())
        let mut in_guards: Vec<MutexGuard<'_, StageInPrio>> = Vec::with_capacity(Priority::NUM);
        for stage_in in self.stage_in.priorities.iter() {
            let guard = stage_in.lock().await;
            in_guards.push(guard);
        }

        // Wake up any blocked pullers by notifying with maximum batch size
        for ig in in_guards.iter_mut() {
            ig.flush();
        }
    }
}

// --- Refill
struct StageRefill {
    /// Waiter for notifications when refilled buffers become available.
    can_refill: Waiter,
    /// Ring buffer reader for pulling recycled empty batches.
    ringbuf_outrefill_r: RingBufferReaderSPMC<WriteBatch>,
    /// Maximum number of buffers that can be allocated
    capacity: usize,
    /// Number of buffers allocated so far (up to capacity).
    allocated: usize,
    /// The batch size
    batch_size: BatchSize,
}

impl StageRefill {
    fn new(
        can_refill: Waiter,
        ringbuf_outrefill_r: RingBufferReaderSPMC<WriteBatch>,
        capacity: usize,
        batch_size: BatchSize,
    ) -> Self {
        Self {
            can_refill,
            ringbuf_outrefill_r,
            capacity,
            allocated: 0,
            batch_size,
        }
    }
    /// Attempts to pull an empty batch for serialization.
    ///
    /// First tries to pull a recycled buffer from the ring. If none are
    /// available and the allocation limit hasn't been reached, allocates a
    /// new buffer.
    ///
    /// # Returns
    ///
    /// - `Some(Batch)` - A recycled or newly allocated empty buffer
    /// - `None` - No buffers available and allocation limit reached
    fn pull(&mut self) -> Option<WriteBatch> {
        match self.ringbuf_outrefill_r.pull() {
            Some(p) => Some(p),
            None => (self.allocated < self.capacity).then_some({
                self.allocated += 1;
                WriteBatch::alloc(self.batch_size)
            }),
        }
    }
}

// --- Out
struct StageOut {
    // Stage in
    ringbuf_inout_r: RingBufferReaderMPSC<WriteBatch>,
    can_pull: Waiter,
    // Stage refill
    ringbuf_outrefill_w: RingBufferWriterSPMC<WriteBatch>,
    can_refill: [Notifier; Priority::NUM],
}

impl StageOut {
    fn refill(&mut self, mut batch: WriteBatch) {
        let priority = batch.qos.priority();
        batch.clear();
        self.ringbuf_outrefill_w.push(priority, batch);
        let _ = self.can_refill[priority].notify();
    }
}

pub(crate) struct PullConfig {
    pub(crate) max_batch_age: Duration,
}

pub(crate) struct PipelineTxOut {
    stage_in: Arc<StageIn>,
    stage_out: StageOut,
}

impl PipelineTxOut {
    pub(crate) async fn pull(&mut self, config: &PullConfig) -> Option<WriteBatch> {
        let mut booked_len: [BatchSize; Priority::NUM] = [0; Priority::NUM];
        let mut backoff = false;
        let mut backoff_num: [BatchSize; Priority::NUM] = [0; Priority::NUM];

        'outer: while !self.stage_in.status.any(status::DISABLED) {
            for p in Priority::ALL {
                match self.stage_out.ringbuf_inout_r.pull_priority(p) {
                    Pull::Some(mut batch) => {
                        let cf = status::congested(batch.qos.priority());
                        let is_congested = status::any(self.stage_in.status.unset(cf), cf);

                        if (is_congested && batch.qos.congestion_control() == CongestionControl::Drop)
                            || batch.drop.as_ref().is_some_and(|df| df.load(Ordering::Relaxed))
                        {
                            self.stage_out.refill(batch);
                            continue 'outer;
                        }

                        batch.write_len();
                        return Some(batch);
                    }
                    Pull::Booked => {
                        if !backoff {
                            backoff = true;
                            self.stage_in.status.set(status::BACKOFF);
                        }

                        // Exponential backoff
                        for _ in 0..1 << backoff_num[p] {
                            tokio::task::yield_now().await;
                        }

                        backoff_num[p] += 1;

                        let (len, first_write) = self.stage_in.backoff[p].load();

                        let old_len = booked_len[p];
                        booked_len[p] = len;

                        let to_pull = match len.cmp(&old_len) {
                            cmp::Ordering::Less => {
                                // We must have a batch in stage out
                                continue 'outer;
                            }
                            cmp::Ordering::Equal => true,
                            cmp::Ordering::Greater => false,
                        } || {
                            let age = LOCAL_EPOCH.elapsed().saturating_sub(first_write);
                            let check = age > config.max_batch_age;
                            if check {
                                self.stage_in.status.set(status::flush(p));
                            }
                            check
                        };

                        if to_pull {
                            if let Some(mut guard) = self.stage_in.priorities[p].try_lock() {
                                guard.flush();
                            }
                        }

                        continue 'outer;
                    }
                    Pull::None => {}
                }
            }

            // Reset backoff
            booked_len = [0; Priority::NUM];
            backoff_num = [0; Priority::NUM];
            backoff = false;
            self.stage_in.status.unset(status::BACKOFF);

            // All priorities require backoff, wait for notification
            let res = self.stage_out.can_pull.wait().await;
            if res.is_err() {
                break;
            }
            // Retry pulling
        }

        None
    }

    pub(crate) fn refill(&mut self, batch: WriteBatch) {
        self.stage_out.refill(batch);
    }

    pub(crate) async fn drain(&mut self) -> Vec<WriteBatch> {
        let mut batches = vec![];

        // Acquire all locks in order to avoid deadlocks (same order as disable())
        let mut stage_in = Vec::with_capacity(Priority::NUM);
        for lock in self.stage_in.priorities.iter() {
            let guard = lock.lock().await;
            stage_in.push(guard);
        }

        // Drain batches from each priority level
        for si in stage_in.iter_mut() {
            si.flush();
        }

        while let Some(b) = self.stage_out.ringbuf_inout_r.pull() {
            batches.push(b);
        }

        batches
    }
}

// All
pub(crate) fn pipeline_tx(capacity: usize, batch_size: BatchSize) -> (PipelineTxIn, PipelineTxOut) {
    // Create event for signaling data availability to receiver
    let (can_pull_notifier, can_pull_waiter) = event::new();

    // Create events for signaling buffer space availability (one per priority level)
    let (can_refill_notifier0, can_refill_waiter0) = event::new();
    let (can_refill_notifier1, can_refill_waiter1) = event::new();
    let (can_refill_notifier2, can_refill_waiter2) = event::new();
    let (can_refill_notifier3, can_refill_waiter3) = event::new();

    // Create ring buffers for each priority level (0 = highest, 7 = lowest)
    let rb_capacity = if capacity.is_power_of_two() {
        capacity
    } else {
        capacity.next_power_of_two()
    };
    let (writers, stage_in_out_r) = ringbuffer_mpsc(rb_capacity);
    let [stage_in_out_w0, stage_in_out_w1, stage_in_out_w2, stage_in_out_w3] = writers;

    let (stage_refill_in_w, readers) = ringbuffer_spmc(rb_capacity);
    let [
        stage_refill_in_r0,
        stage_refill_in_r1,
        stage_refill_in_r2,
        stage_refill_in_r3,
    ] = readers;

    let stage_in = StageIn {
        priorities: [
            Mutex::new(StageInPrio {
                seq_num: SeqNumPool::new(),
                ringbuf_inout_w: stage_in_out_w0,
                stage_refill: StageRefill::new(can_refill_waiter0, stage_refill_in_r0, capacity, batch_size),
            }),
            Mutex::new(StageInPrio {
                seq_num: SeqNumPool::new(),
                ringbuf_inout_w: stage_in_out_w1,
                stage_refill: StageRefill::new(can_refill_waiter1, stage_refill_in_r1, capacity, batch_size),
            }),
            Mutex::new(StageInPrio {
                seq_num: SeqNumPool::new(),
                ringbuf_inout_w: stage_in_out_w2,
                stage_refill: StageRefill::new(can_refill_waiter2, stage_refill_in_r2, capacity, batch_size),
            }),
            Mutex::new(StageInPrio {
                seq_num: SeqNumPool::new(),
                ringbuf_inout_w: stage_in_out_w3,
                stage_refill: StageRefill::new(can_refill_waiter3, stage_refill_in_r3, capacity, batch_size),
            }),
        ],
        backoff: [
            AtomicBackoff::new(),
            AtomicBackoff::new(),
            AtomicBackoff::new(),
            AtomicBackoff::new(),
        ],
        can_pull: can_pull_notifier,
        status: Status::new(),
    };

    let ptxin = PipelineTxIn {
        stage_in: Arc::new(stage_in),
    };

    let ptxout = PipelineTxOut {
        stage_in: ptxin.stage_in.clone(),
        stage_out: StageOut {
            ringbuf_inout_r: stage_in_out_r,
            can_pull: can_pull_waiter,
            ringbuf_outrefill_w: stage_refill_in_w,
            can_refill: [
                can_refill_notifier0,
                can_refill_notifier1,
                can_refill_notifier2,
                can_refill_notifier3,
            ],
        },
    };

    (ptxin, ptxout)
}

#[cfg(test)]
mod tests {
    use tokio::time::timeout;

    use super::*;
    use crate::{
        buffers::Bytes,
        protocol::{BatchSize, CongestionControl, Priority, QoS},
    };

    const TIMEOUT: Duration = Duration::from_secs(3);
    const BATCH_SIZE: BatchSize = 8192;
    const CAPACITY: usize = 4;

    fn create_test_message(payload_size: usize) -> Bytes {
        Bytes::from(vec![0u8; payload_size])
    }

    fn create_push_config(qos: QoS) -> PushConfig {
        PushConfig {
            qos,
            timeout: Duration::from_secs(1),
            timeout_batch: Duration::from_secs(1),
        }
    }

    fn create_pull_config() -> PullConfig {
        PullConfig {
            max_batch_age: Duration::from_secs(1),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_basic_push_pull() {
        let (tx_in, mut tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);
        let pull_config = create_pull_config();

        let qos = QoS::DEFAULT;
        let push_config = create_push_config(qos);
        let msg = create_test_message(100);

        // Push a message
        timeout(TIMEOUT, tx_in.push(&msg, &push_config)).await.unwrap().unwrap();

        // Pull the batch
        let batch = timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap().unwrap();

        // Batch should contain data
        assert!(batch.frame.len() > msg.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_priority_ordering() {
        let (tx_in, mut tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);
        let pull_config = create_pull_config();

        // Push messages with different priorities
        let msg = create_test_message(100);
        for p in Priority::ALL.iter().rev() {
            let qos = QoS::DEFAULT.with_priority(*p);
            let push_config = create_push_config(qos);
            timeout(TIMEOUT, tx_in.push(&msg, &push_config)).await.unwrap().unwrap();
        }

        // Pull batches - should come out in priority order (High, Medium, Low, Background)
        for p in Priority::ALL {
            let batch = timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap().unwrap();
            assert_eq!(batch.qos.priority(), p);
            tx_out.refill(batch);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_congestion_drop_refill() {
        let (tx_in, _tx_out) = pipeline_tx(1, BATCH_SIZE); // Very small capacity

        let msg = create_test_message(2000);

        // Fill the pipeline with Block messages to cause congestion
        let qos_block = QoS::DEFAULT.with_congestion_control(CongestionControl::Block);
        let push_config_block = create_push_config(qos_block);

        // Push messages to fill capacity
        for _ in 0..3 {
            timeout(TIMEOUT, tx_in.push(&msg, &push_config_block))
                .await
                .unwrap()
                .unwrap();
        }

        // Try to push a Drop message - should be dropped immediately
        let qos_drop = QoS::DEFAULT.with_congestion_control(CongestionControl::Drop);
        let push_config_drop = create_push_config(qos_drop);

        // Only one batch available (Block) and no refill
        let err = timeout(TIMEOUT, tx_in.push(&msg, &push_config_drop))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, SendError::Timeout);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_congestion_drop_full() {
        let (tx_in, _tx_out) = pipeline_tx(1, BATCH_SIZE); // Very small capacity

        let msg = create_test_message(2000);

        // Try to push a Drop message - should be dropped immediately
        let qos_drop = QoS::DEFAULT.with_congestion_control(CongestionControl::Drop);
        let push_config_drop = create_push_config(qos_drop);

        // Push messages to fill capacity
        for _ in 0..4 {
            timeout(TIMEOUT, tx_in.push(&msg, &push_config_drop))
                .await
                .unwrap()
                .unwrap();
        }

        // Only one batch available (Block) and no refill
        let err = timeout(TIMEOUT, tx_in.push(&msg, &push_config_drop))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, SendError::Timeout);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_congestion_drop_fragment() {
        let (tx_in, mut tx_out) = pipeline_tx(2, BATCH_SIZE); // Very small capacity        

        // Try to push a Drop message - should be dropped immediately
        let qos_drop = QoS::DEFAULT.with_congestion_control(CongestionControl::Drop);
        let push_config_drop = create_push_config(qos_drop);

        // Push messages to fill capacity
        let msg = create_test_message(2 * BATCH_SIZE as usize);
        let err = timeout(TIMEOUT, tx_in.push(&msg, &push_config_drop))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, SendError::Timeout);

        let pull_config = create_pull_config();
        timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap_err();

        // Only one batch available (Block) and no refill
        let msg = create_test_message(BATCH_SIZE as usize / 2);
        timeout(TIMEOUT, tx_in.push(&msg, &push_config_drop))
            .await
            .unwrap()
            .unwrap();
        let b = timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap().unwrap();
        assert!(b.len() as usize > msg.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_express_mode() {
        let (tx_in, mut tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);
        let pull_config = create_pull_config();

        // Push an express message (should flush immediately)
        let qos = QoS::DEFAULT
            .with_congestion_control(CongestionControl::Block)
            .with_express(true); // express = true
        let push_config = create_push_config(qos);
        let msg = create_test_message(100);

        for _ in 0..CAPACITY {
            timeout(TIMEOUT, tx_in.push(&msg, &push_config)).await.unwrap().unwrap();
        }
        let err = timeout(TIMEOUT, tx_in.push(&msg, &push_config))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, SendError::Timeout);

        // Should be able to pull immediately
        for _ in 0..CAPACITY {
            let batch = timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap().unwrap();
            assert!(batch.qos.express());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_pipeline_closed() {
        let (tx_in, tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);

        // Drop the output side to close the pipeline
        drop(tx_out);

        let qos = QoS::DEFAULT;
        let push_config = create_push_config(qos);
        let msg = create_test_message(100);

        // First push might succeed (batch available)
        let _ = timeout(TIMEOUT, tx_in.push(&msg, &push_config)).await.unwrap();

        // Subsequent pushes should fail with Closed error
        let result = tx_in.push(&msg, &push_config).await;
        assert_eq!(result, Err(SendError::Closed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_disable_pipeline() {
        let (tx_in, mut tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);

        // Disable the pipeline
        timeout(TIMEOUT, tx_in.disable()).await.unwrap();

        // Pull should return None (pipeline disabled)
        let pull_config = create_pull_config();
        let result = timeout(TIMEOUT, tx_out.pull(&pull_config)).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_drain_pipeline() {
        let (tx_in, mut tx_out) = pipeline_tx(CAPACITY, BATCH_SIZE);

        // Push several messages
        let qos = QoS::DEFAULT;
        let push_config = create_push_config(qos);
        let msg = create_test_message(100);
        for _ in 0..3 {
            timeout(TIMEOUT, tx_in.push(&msg, &push_config)).await.unwrap().unwrap();
        }

        // Drain the pipeline
        let batches = timeout(TIMEOUT, tx_out.drain()).await.unwrap();

        // Should have drained all batches
        assert!(!batches.is_empty());
    }
}
