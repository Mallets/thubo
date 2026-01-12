/// Congestion control strategy for handling network congestion.
///
/// Determines how messages are handled when network queues are full. This setting
/// works in conjunction with [`Priority`](`super::Priority`) to control message delivery guarantees.
///
/// # Choosing a Strategy
///
/// - **Use [`Block`](`CongestionControl::Block`)** when message delivery is more important than latency
/// - **Use [`Drop`](`CongestionControl::Drop`)** when low latency is more important than delivery guarantee
///
/// The choice depends on your application's requirements and the nature of the data being sent.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum CongestionControl {
    /// Block strategy - Wait for queue space to become available.
    ///
    /// **This is the default strategy.** When a message encounters a full priority queue,
    /// the sender blocks until space becomes available. This guarantees delivery but may
    /// introduce latency during congestion.
    ///
    /// # Behavior
    ///
    /// - Send operations block (async await) until queue space is available
    /// - Messages are never dropped due to congestion
    /// - Provides reliable, in-order delivery within each priority level
    /// - May cause backpressure to propagate to the application
    ///
    /// # When to Use
    ///
    /// Use Block when:
    /// - **Reliability is critical**: Every message must be delivered (e.g., financial transactions, control commands,
    ///   state updates)
    /// - **Data integrity matters**: Loss would cause inconsistency or require complex recovery (e.g., database
    ///   replication, configuration updates)
    /// - **Latency spikes are acceptable**: Temporary delays are preferable to data loss
    Block = 0,

    /// Drop strategy - Allow message dropping under congestion.
    ///
    /// When a message encounters a full priority queue, it is immediately dropped with
    /// [`SendError::Dropped`](`crate::SendError::Dropped`). This prevents blocking and maintains
    /// low latency at the cost of potential message loss.
    ///
    /// The Drop strategy employs a two-stage dropping mechanism:
    /// 1. **Tail dropping**: New messages are initially dropped when attempting to send into a full queue
    /// 2. **Head dropping**: After tail dropping has occurred and congestion persists, stale messages already queued
    ///    may be dropped to make room for newer messages
    ///
    /// This approach prioritizes message freshness during sustained congestion - older
    /// queued data is replaced with newer data, ensuring that only the most current
    /// information is transmitted. This is particularly valuable for time-sensitive
    /// applications where stale information has diminishing value.
    ///
    /// # Behavior
    ///
    /// - Send operations fail immediately with [`SendError::Dropped`](`crate::SendError::Dropped`) when queue is full
    /// - No waiting or backpressure - fails fast
    /// - Priority level is marked as congested after dropping
    /// - During sustained congestion, stale queued messages may be dropped to favor newer messages
    /// - Maintains low latency for successfully sent messages
    ///
    /// # When to Use
    ///
    /// Use Drop when:
    /// - **Freshness matters more than completeness**: Stale data is less valuable than current data (e.g., live sensor
    ///   readings, video frames, mouse positions)
    /// - **Data has natural redundancy**: Missing one message doesn't break functionality (e.g., periodic telemetry,
    ///   real-time dashboards)
    /// - **Latency is critical**: Delays are more harmful than missing messages (e.g., gaming, live streaming, audio)
    /// - **Recovery is application-level**: The application handles retries or missing data
    Drop = 1,
}

impl Default for CongestionControl {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl CongestionControl {
    /// Default congestion control strategy (Block)
    pub const DEFAULT: Self = Self::Block;

    /// Creates a CongestionControl from a boolean value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `v` represents a valid CongestionControl
    /// value.
    /// - `false` maps to Block
    /// - `true` maps to Drop
    pub(crate) const unsafe fn from_bool(v: bool) -> Self {
        unsafe { core::mem::transmute(v) }
    }

    /// Generates a random congestion control strategy for testing.
    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        use rand::Rng;

        let mut rng = rand::rng();
        if rng.random_bool(0.5) { Self::Drop } else { Self::Block }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_bool() {
        // false maps to Block
        let block = unsafe { CongestionControl::from_bool(false) };
        assert_eq!(block, CongestionControl::Block);

        // true maps to Drop
        let drop = unsafe { CongestionControl::from_bool(true) };
        assert_eq!(drop, CongestionControl::Drop);
    }
}
