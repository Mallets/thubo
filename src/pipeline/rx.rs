use std::{sync::Arc, time::Duration};

use thiserror::Error;

use super::{
    LOCAL_EPOCH,
    ringbuf::{RingBufferReaderSPSC, RingBufferWriterSPSC, ringbuffer_spsc},
};
use crate::{
    buffers::{
        Bytes, BytesPos,
        reader::{Reader, SeekableReader},
    },
    codec::{RCodec, ThuboCodec},
    pipeline::{Status, status},
    protocol::{
        FragmentKind, Message, MessageBody,
        core::{CongestionControl, Priority, QoS, SeqNum},
    },
    sync::{Notifier, Waiter, event},
};

struct ReadBatch {
    bytes: Bytes,
    qos: QoS,
    pos: BytesPos,
}

pub(crate) fn pipeline_rx(capacity: usize) -> (PipelineRxIn, PipelineRxOut) {
    // Create event for signaling data availability to receiver
    let (can_recv_notifier, can_recv_waiter) = event::new();

    // Create events for signaling buffer space availability (one per priority
    // level)
    let (can_send_notifier0, can_send_waiter0) = event::new();
    let (can_send_notifier1, can_send_waiter1) = event::new();
    let (can_send_notifier2, can_send_waiter2) = event::new();
    let (can_send_notifier3, can_send_waiter3) = event::new();

    // Create ring buffers for each priority level (0 = highest, 7 = lowest)
    let (writer, reader) = ringbuffer_spsc(capacity);

    let ring_buf_in = Box::new((
        writer,
        [can_send_waiter0, can_send_waiter1, can_send_waiter2, can_send_waiter3],
    ));

    let ring_buf_out = Box::new((
        reader,
        [
            can_send_notifier0,
            can_send_notifier1,
            can_send_notifier2,
            can_send_notifier3,
        ],
    ));

    // Create defrag buffers
    let block = Box::new([
        DefragBuffer::new(),
        DefragBuffer::new(),
        DefragBuffer::new(),
        DefragBuffer::new(),
    ]);
    let drop = block.clone();

    let status = Arc::new(Status::new());

    let sender = PipelineRxIn {
        can_recv: can_recv_notifier,
        ring_buf: ring_buf_in,
        defrag: Defragmentation { block, drop },
        status: status.clone(),
    };

    let receiver = PipelineRxOut {
        can_recv: can_recv_waiter,
        ring_buf: ring_buf_out,
        status,
    };

    (sender, receiver)
}

/// Error returned when a receive operation fails.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecvError {
    /// Message deserialization failed due to corrupted or invalid data.
    ///
    /// This error occurs when the receiver successfully reads data from the
    /// channel but cannot decode it into the expected message type. Common
    /// causes include:
    /// - Data corruption during transmission
    /// - Invalid message format that doesn't match the expected schema
    ///
    /// When this error occurs, the problematic batch is discarded and cannot be
    /// recovered.
    #[error("Message deserialization failed due to corrupted or invalid data")]
    DecodingFailed,

    /// No messages received within the configured read timeout.
    ///
    /// This error occurs when [`recv()`](`crate::api::Receiver::recv`) waits for a message
    /// but no data arrives within the timeout period configured via
    /// [`ReceiverTask::set_read_timeout()`](`crate::api::ReceiverTask::set_read_timeout`).
    ///
    /// Common causes:
    /// - Network connectivity issues or high latency
    /// - Sender stopped sending messages
    /// - Timeout configured too aggressively for the network conditions
    ///
    /// This is **not** a terminal error - subsequent receive operations may succeed if
    /// messages arrive later. The receiver remains open and functional.
    ///
    /// # Default Timeout
    ///
    /// The default read timeout is 10 seconds.
    #[error("Timed out while waiting for new data")]
    Timeout,

    /// The channel has been closed and no more messages can be received.
    ///
    /// This error indicates that the sender side of the channel has been
    /// dropped or explicitly closed, or the underlying connection has been
    /// lost. No further receive operations will succeed after this error.
    ///
    /// This is a terminal state - once closed, the receiver cannot be reopened.
    #[error("The channel has been closed and no more messages can be received")]
    Closed,
}

pub(crate) struct PipelineRxOut {
    can_recv: Waiter,
    ring_buf: Box<(RingBufferReaderSPSC<ReadBatch>, [Notifier; Priority::NUM])>,
    status: Arc<Status>,
}

impl PipelineRxOut {
    pub(crate) async fn pull(&mut self, config: &PullConfig) -> Result<(Bytes, QoS), RecvError> {
        loop {
            // Try to pull a message from the priority queues (non-blocking)
            if let Some((bytes, qos)) = self.try_pull() {
                return Ok((bytes, qos));
            }

            // No messages currently available - wait for notification from StageIn
            // that new data has arrived, with a timeout to detect stalled connections
            match tokio::time::timeout(config.timeout, self.can_recv.wait()).await {
                Ok(Ok(())) => {
                    // Notified that new messages are available - retry pulling
                    // (loop continues)
                }
                Ok(Err(_)) => {
                    // Channel closed - sender dropped or connection lost
                    break Err(RecvError::Closed);
                }
                Err(_) => {
                    // Timeout expired - no messages received within configured duration
                    break Err(RecvError::Timeout);
                }
            }
        }
    }

    pub(crate) fn try_pull(&mut self) -> Option<(Bytes, QoS)> {
        let (ring_buf, can_send) = self.ring_buf.as_mut();

        // Inner loop: process all batches at this priority level before moving to next
        while let Some(batch) = ring_buf.peek_mut() {
            let priority = batch.qos.priority();

            let cf = status::congested(priority);
            let is_congested = status::any(self.status.unset(cf), cf);
            if is_congested && batch.qos.congestion_control() == CongestionControl::Drop {
                let _ = ring_buf.pull();
                let _ = can_send[priority].notify();
                // Drop and repull
                continue;
            }

            // We have a ReadBatch - try to deserialize the next message from it
            let codec = ThuboCodec::new();
            let mut reader = batch.bytes.reader();

            // Seek to the position where we left off (for multi-message batches)
            // and check if more data is available to read
            if reader.seek(batch.pos) {
                if let Ok(t) = codec.read(&mut reader) {
                    // Successfully deserialized a message
                    let qos = batch.qos;
                    // Update position so next call continues from here
                    batch.pos = reader.mark();
                    return Some((t, qos));
                }
            }

            // No more messages in this batch (or deserialization failed)
            // Remove it from the queue and notify sender that space is available
            let _ = ring_buf.pull();
            let _ = can_send[priority].notify();
            // Continue loop to check next batch at this same priority level
        }

        // No messages available at any priority level
        None
    }
}

/// A ring buffer writer paired with a waiter for buffer space notifications.
pub(crate) struct PipelineRxIn {
    can_recv: Notifier,
    ring_buf: Box<(RingBufferWriterSPSC<ReadBatch>, [Waiter; Priority::NUM])>,
    defrag: Defragmentation,
    status: Arc<Status>,
}

struct Defragmentation {
    block: Box<[DefragBuffer; Priority::NUM]>,
    drop: Box<[DefragBuffer; Priority::NUM]>,
}

pub(crate) struct PushConfig {
    pub(crate) frag_max_size: usize,
    pub(crate) timeout_drop: Duration,
    pub(crate) timeout_block: Duration,
}

impl PipelineRxIn {
    pub(crate) async fn push(&mut self, bytes: Bytes, config: &PushConfig) -> Result<bool, SendError> {
        let codec = ThuboCodec::new();

        // Decode the message header from the batch
        let mut reader = bytes.reader();

        let Ok(message): Result<Message, _> = codec.read(&mut reader) else {
            return Err(SendError::DecodingFailed); // Decoding failed - skip this batch
        };
        let qos = message.header.qos();

        // Create an Chunk view of the message payload (after the header)
        let start = bytes.len() - reader.remaining();

        let mut batch = match message.body {
            // Complete message in a single batch
            MessageBody::Frame(_) => {
                let pos = reader.mark();
                ReadBatch { bytes, qos, pos }
            }

            // Fragment of a larger message
            MessageBody::Fragment(f) => {
                // Select the appropriate defragmentation buffer based on congestion control
                let defrag = match qos.congestion_control() {
                    CongestionControl::Block => &mut self.defrag.block,
                    CongestionControl::Drop => &mut self.defrag.drop,
                };
                let dbuf = &mut defrag[qos.priority() as usize];

                // Initialize defragmentation buffer if this is the first fragment
                match f.kind() {
                    FragmentKind::First => {
                        dbuf.clear();
                        dbuf.sync(message.header.seq_num, f.frag_id);
                        // Add fragment to the defragmentation buffer
                        let bytes = bytes.view(start..).ok_or(SendError::InternalError)?;
                        dbuf.extend(message.header.seq_num, f.frag_id, bytes, config.frag_max_size)?;
                        return Ok(true);
                    }
                    FragmentKind::More => {
                        // Add fragment to the defragmentation buffer
                        let bytes = bytes.view(start..).ok_or(SendError::InternalError)?;
                        dbuf.extend(message.header.seq_num, f.frag_id, bytes, config.frag_max_size)
                            .inspect_err(|_| dbuf.clear())?;
                        return Ok(true);
                    }
                    FragmentKind::Last => {
                        // Add fragment to the defragmentation buffer
                        let bytes = bytes.view(start..).ok_or(SendError::InternalError)?;
                        dbuf.extend(message.header.seq_num, f.frag_id, bytes, config.frag_max_size)?;
                        ReadBatch {
                            bytes: dbuf.take(),
                            qos,
                            pos: BytesPos::zero(),
                        }
                    }
                }
            }
        };

        let priority = batch.qos.priority();
        let congestion_control = batch.qos.congestion_control();

        // Get the appropriate writer and waiter for this priority level
        let start = LOCAL_EPOCH.elapsed();
        let compute_timeout = |to: Duration| {
            LOCAL_EPOCH
                .elapsed()
                .checked_sub(start)
                .and_then(|elapsed| to.checked_sub(elapsed))
                .ok_or(SendError::Timeout)
        };

        let writer = &mut self.ring_buf.0;
        let can_send = &self.ring_buf.1[priority];
        loop {
            match writer.push(priority, batch) {
                // Buffer is full, returned the message back
                Some(zb) => {
                    match congestion_control {
                        CongestionControl::Block => {
                            let timeout = compute_timeout(config.timeout_block)?;
                            // Wait for space to become available
                            match tokio::time::timeout(timeout, can_send.wait()).await {
                                Ok(Ok(())) => { /* Retry with the returned buffer */ }
                                Ok(Err(_)) => break Err(SendError::ChannelClosed), // Channel closed
                                Err(_) => {
                                    self.status.set(status::congested(priority));
                                }
                            }
                        }
                        CongestionControl::Drop => {
                            if self.status.any(status::congested(priority)) {
                                // Currently congested - directly drop the message without waiting
                                break Ok(false);
                            }
                            let timeout = compute_timeout(config.timeout_drop)?;
                            match tokio::time::timeout(timeout, can_send.wait()).await {
                                Ok(Ok(_)) => { /* Retry with the returned buffer */ }
                                Ok(Err(_)) => break Err(SendError::ChannelClosed),
                                Err(_) => {
                                    self.status.set(status::congested(priority));
                                    break Ok(false);
                                }
                            }
                        }
                    }
                    batch = zb;
                }
                // Successfully pushed to buffer
                None => {
                    // Notify receiver that data is available
                    self.can_recv.notify().map_err(|_| SendError::ChannelClosed)?;
                    break Ok(true);
                }
            }
        }
    }
}

pub(crate) struct PullConfig {
    pub(crate) timeout: Duration,
}

// ================================================================================================
// DefragBuffer - Message Defragmentation Support
// ================================================================================================
/// Buffer for reassembling fragmented messages.
///
/// Large messages that exceed the batch size are split into fragments during
/// transmission. This buffer reassembles fragments back into complete messages
/// by:
/// 1. Tracking the expected sequence number and fragment ID
/// 2. Accumulating fragment payloads in order
/// 3. Validating fragment continuity
/// 4. Enforcing capacity limits
///
/// # Fragment Validation
///
/// Each fragment must have:
/// - The expected sequence number (increments with each is_fragmented)
/// - The expected fragment ID (same for all fragments of a message)
///
/// Out-of-order or duplicate fragments cause the buffer to be cleared.
#[derive(Debug, Clone)]
pub(super) struct DefragBuffer {
    /// Expected sequence number for the next fragment.
    seq_num: SeqNum,
    /// Fragment ID for the current message being reassembled.
    frag_id: SeqNum,
    /// Accumulated message data from fragments.
    buffer: Bytes,
}

/// Errors that can occur when pushing fragments to the defragmentation buffer.
#[derive(Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SendError {
    /// Invalid format
    DecodingFailed,
    /// Fragment has an unexpected sequence number (out of order or duplicate).
    InvalidSeqNum,
    /// Fragment has a different fragment ID than expected (wrong message).
    InvalidFragId,
    /// Adding this fragment would exceed the capacity limit.
    CapacityLimit,
    /// Channel closed
    ChannelClosed,
    /// Timeout
    Timeout,
    /// Internal Error
    InternalError,
}

impl DefragBuffer {
    /// Creates a new defragmentation buffer with the specified capacity.
    pub(crate) fn new() -> Self {
        DefragBuffer {
            seq_num: SeqNum::new(0),
            frag_id: SeqNum::new(0),
            buffer: Bytes::new(),
        }
    }

    /// Clears all accumulated fragments from the buffer.
    #[inline(always)]
    pub(super) fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Synchronizes the buffer to expect a new message.
    ///
    /// Sets the expected sequence number and fragment ID for the first fragment
    /// of a new message. Called when starting defragmentation of a new message.
    #[inline(always)]
    pub(super) fn sync(&mut self, seq_num: SeqNum, frag_id: SeqNum) {
        self.seq_num = seq_num;
        self.frag_id = frag_id;
    }

    /// Pushes a fragment into the defragmentation buffer.
    ///
    /// Validates the fragment's sequence number and fragment ID, then appends
    /// its payload to the buffer. The sequence number is automatically
    /// incremented to expect the next fragment.
    ///
    /// # Returns
    ///
    /// - `Ok(())` - Fragment was successfully added
    /// - `Err(SendError)` - Validation failed or capacity exceeded (buffer is cleared)
    ///
    /// # Fragment Ordering
    ///
    /// Fragments must arrive in order with consecutive sequence numbers.
    /// Out-of-order fragments cause the buffer to be cleared and return an
    /// error.
    pub(super) fn extend(
        &mut self,
        seq_num: SeqNum,
        frag_id: SeqNum,
        bytes: Bytes,
        capacity: usize,
    ) -> Result<(), SendError> {
        // Validate sequence number
        if seq_num != self.seq_num {
            return Err(SendError::InvalidSeqNum);
        }

        // Validate fragment ID
        if frag_id != self.frag_id {
            return Err(SendError::InvalidFragId);
        }

        // Check capacity limit
        let new_len = self.buffer.len() + bytes.len();
        if new_len > capacity {
            return Err(SendError::CapacityLimit);
        }

        // Accept the fragment
        self.seq_num.increment();
        self.buffer.extend(bytes);

        Ok(())
    }

    /// Takes the complete reassembled message from the buffer.
    ///
    /// Removes and returns all accumulated fragments as a complete message.
    /// The buffer is left empty after this operation.
    ///
    /// # Returns
    ///
    /// The complete reassembled message as a `Bytes`.
    pub(super) fn take(&mut self) -> Bytes {
        let mut buf = Bytes::new();
        core::mem::swap(&mut buf, &mut self.buffer);
        buf
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;
    use crate::{
        buffers::{Bytes, writer::HasWriter},
        codec::{ThuboCodec, WCodec},
        protocol::{
            BatchSize, Message, MessageBody, MessageHeader, QoS,
            core::{Priority, SeqNum},
            fragment::{FragmentHeader, FragmentKind},
            frame::FrameHeader,
        },
    };

    const TIMEOUT: Duration = Duration::from_secs(3);
    const CAPACITY: usize = 4;

    fn create_push_config() -> PushConfig {
        PushConfig {
            frag_max_size: BatchSize::MAX as usize,
            timeout_drop: Duration::from_millis(100),
            timeout_block: Duration::from_secs(1),
        }
    }

    fn create_pull_config() -> PullConfig {
        PullConfig {
            timeout: Duration::from_secs(1),
        }
    }

    fn encode_frame_message(payload: &[u8], qos: QoS, seq_num: u32) -> Bytes {
        let codec = ThuboCodec::new();
        let message = Message::new(
            MessageHeader::new(qos, SeqNum::new(seq_num)),
            MessageBody::Frame(FrameHeader::new()),
        );

        let mut bytes: Vec<u8> = vec![];
        let mut writer = bytes.writer();
        codec.write(&mut writer, &message).unwrap();
        codec.write(&mut writer, payload).unwrap();
        Bytes::from(bytes)
    }

    fn encode_fragment_message(
        payload: &[u8],
        qos: QoS,
        seq_num: u32,
        frag_id: u32,
        kind: FragmentKind,
        frag_len: usize,
    ) -> Bytes {
        let codec = ThuboCodec::new();
        let message = Message::new(
            MessageHeader::new(qos, SeqNum::new(seq_num)),
            MessageBody::Fragment(FragmentHeader::new(kind, SeqNum::new(frag_id))),
        );

        let mut bytes: Vec<u8> = vec![];
        let mut writer = bytes.writer();
        codec.write(&mut writer, &message).unwrap();
        if let FragmentKind::First = kind {
            codec.write(&mut writer, frag_len).unwrap();
        }
        bytes.extend_from_slice(payload);
        Bytes::from(bytes)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_basic_push_pull() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();
        let pull_config = create_pull_config();

        let qos = QoS::DEFAULT;
        let payload = b"Hello, World!";
        let encoded = encode_frame_message(payload, qos, 1);

        // Push a message
        timeout(TIMEOUT, rx_in.push(encoded, &push_config))
            .await
            .unwrap()
            .unwrap();

        // Pull the message
        let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();

        assert_eq!(pulled_qos, qos);
        // Compare payload bytes
        assert!(bytes.len() >= payload.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_priority_ordering() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();
        let pull_config = create_pull_config();

        // Push messages with different priorities in reverse order
        for p in Priority::ALL.iter().rev().copied() {
            let qos = QoS::DEFAULT.with_priority(p);
            let payload = format!("Message {}", p);
            let encoded = encode_frame_message(payload.as_bytes(), qos, p as u32);
            rx_in.push(encoded, &push_config).await.unwrap();
        }

        // Pull messages - should come out in priority order (High, Medium, Low, Background)
        for p in Priority::ALL {
            let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();
            let payload = format!("Message {}", p);
            assert_eq!(bytes.to_vec().as_slice(), payload.as_bytes());
            assert_eq!(pulled_qos.priority(), p);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_fragmentation() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();
        let pull_config = create_pull_config();

        let qos = QoS::DEFAULT;
        let mut seq_num = SeqNum::new(0);
        let frag_id = 42;

        // Send a fragmented message in 3 parts
        let part1 = b"Hello, ";
        let part2 = b"fragmented ";
        let part3 = b"world!";
        let frag_len = part1.len() + part2.len() + part3.len();

        // First fragment
        let encoded1 = encode_fragment_message(part1, qos, seq_num.get(), frag_id, FragmentKind::First, frag_len);
        let ok = timeout(TIMEOUT, rx_in.push(encoded1, &push_config))
            .await
            .unwrap()
            .unwrap();
        assert!(ok); // Fragment accepted but not complete

        // Middle fragment
        seq_num = seq_num.next();
        let encoded2 = encode_fragment_message(part2, qos, seq_num.get(), frag_id, FragmentKind::More, frag_len);
        let ok = timeout(TIMEOUT, rx_in.push(encoded2, &push_config))
            .await
            .unwrap()
            .unwrap();
        assert!(ok); // Fragment accepted but not complete

        // Last fragment - message should now be complete
        seq_num = seq_num.next();
        let encoded3 = encode_fragment_message(part3, qos, seq_num.get(), frag_id, FragmentKind::Last, frag_len);
        let ok = timeout(TIMEOUT, rx_in.push(encoded3, &push_config))
            .await
            .unwrap()
            .unwrap();
        assert!(ok);

        // Pull the reassembled message
        let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();

        assert_eq!(pulled_qos, qos);
        // The reassembled message should contain all three parts
        assert_eq!(frag_len, bytes.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_pipeline_closed() {
        let (mut rx_in, rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();

        // Drop the output side to close the pipeline
        drop(rx_out);

        let qos = QoS::DEFAULT;
        let payload = b"Test message";
        let encoded = encode_frame_message(payload, qos, 1);

        // Push should eventually fail after the channel is closed
        tokio::time::sleep(Duration::from_millis(50)).await;

        let result = timeout(Duration::from_millis(200), rx_in.push(encoded, &push_config)).await;
        // Should either timeout or get channel closed error
        assert!(result.is_err() || result.unwrap().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_multiple_messages() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();
        let pull_config = create_pull_config();

        let qos = QoS::DEFAULT;

        // Push multiple messages
        for i in 0..CAPACITY as u32 {
            let payload = format!("Message {}", i);
            let encoded = encode_frame_message(payload.as_bytes(), qos, i + 1);
            let ok = timeout(TIMEOUT, rx_in.push(encoded, &push_config))
                .await
                .unwrap()
                .unwrap();
            assert!(ok);
        }

        // The queue is congested
        let i = CAPACITY as u32;
        let payload = format!("Message {}", i);
        let encoded = encode_frame_message(payload.as_bytes(), qos, i + 1);
        let err = timeout(TIMEOUT, rx_in.push(encoded, &push_config))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(err, SendError::Timeout);
        assert!(rx_in.status.any(status::congested(qos.priority())));

        // Pull all messages
        for i in 0..CAPACITY {
            let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();
            let payload = format!("Message {}", i);
            assert_eq!(bytes.to_chunk().as_slice(), payload.as_bytes());
            assert_eq!(pulled_qos, qos);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_mixed_priorities() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();
        let pull_config = create_pull_config();

        // Push messages alternating between high and low priority
        const N: u32 = 4;

        for i in 0..N {
            let priority = if i % 2 == 0 {
                Priority::High
            } else {
                Priority::Background
            };
            let qos = QoS::DEFAULT.with_priority(priority);
            let payload = format!("Message {} (priority {:?})", i, priority);
            let encoded = encode_frame_message(payload.as_bytes(), qos, i + 1);
            rx_in.push(encoded, &push_config).await.unwrap();
        }

        // All High priority messages should come out first
        let priority = Priority::High;
        for i in 0..N / 2 {
            let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();
            assert_eq!(pulled_qos.priority(), priority);
            let payload = format!("Message {} (priority {:?})", 2 * i, pulled_qos.priority());
            assert_eq!(bytes.to_chunk().as_slice(), payload.as_bytes());
        }

        // Then all background messages
        let priority = Priority::Background;
        for i in 0..N / 2 {
            let (bytes, pulled_qos) = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap();
            assert_eq!(pulled_qos.priority(), priority);
            let payload = format!("Message {} (priority {:?})", 2 * i + 1, pulled_qos.priority());
            assert_eq!(bytes.to_chunk().as_slice(), payload.as_bytes());
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_try_pull_empty() {
        let (_rx_in, mut rx_out) = pipeline_rx(CAPACITY);

        // try_pull on empty pipeline should return None
        let result = rx_out.try_pull();
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_pull_timeout() {
        let (_rx_in, mut rx_out) = pipeline_rx(CAPACITY);

        let pull_config = PullConfig {
            timeout: Duration::from_millis(100),
        };

        // Pull with timeout should timeout when no messages available
        let err = timeout(TIMEOUT, rx_out.pull(&pull_config)).await.unwrap().unwrap_err();
        assert_eq!(err, RecvError::Timeout);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_drop_head() {
        let (mut rx_in, mut rx_out) = pipeline_rx(CAPACITY);
        let push_config = create_push_config();

        let qos = QoS::DEFAULT.with_congestion_control(CongestionControl::Drop);

        for i in 0..CAPACITY as u32 {
            let payload = format!("My payload {}", i);
            let encoded = encode_frame_message(payload.as_bytes(), qos, i);
            timeout(TIMEOUT, rx_in.push(encoded, &push_config))
                .await
                .unwrap()
                .unwrap();
        }

        // Trigger congestion
        let payload = format!("My payload {}", CAPACITY);
        let encoded = encode_frame_message(payload.as_bytes(), qos, CAPACITY as u32);
        let pushed = timeout(TIMEOUT, rx_in.push(encoded, &push_config))
            .await
            .unwrap()
            .unwrap();
        assert!(!pushed);

        // try_pull on empty pipeline should return None
        for _ in 0..(CAPACITY - 1) as u32 {
            let _ = rx_out.try_pull().unwrap();
        }
        let result = rx_out.try_pull();
        assert!(result.is_none());
    }
}
