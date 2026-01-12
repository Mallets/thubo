use core::fmt;

use super::{CongestionControl, priority::Priority};

/// Quality of Service (QoS) configuration for network messages.
///
/// [`QoS`] combines three independent settings that control message delivery
/// behavior:
///
/// - **[`Priority`]**: Determines processing order (4 levels from [`Priority::High`] to [`Priority::Background`]). Uses
///   strict priority scheduling where higher priority messages are always processed first.
/// - **Express delivery**: When enabled, messages are sent immediately without batching, providing lower latency at the
///   cost of reduced throughput efficiency.
/// - **[`CongestionControl`]**: Determines behavior under congestion - either block and wait for capacity
///   ([`CongestionControl::Block`]) or drop the message ([`CongestionControl::Drop`]).
///
/// # Examples
///
/// ## Creating QoS Configurations
///
/// ```
/// use thubo::{CongestionControl, Priority, QoS};
///
/// // Default configuration: Priority::Medium, non-express, Block on congestion
/// let default_qos = QoS::default();
/// assert_eq!(default_qos.priority(), Priority::Medium);
/// assert!(!default_qos.express());
/// assert_eq!(default_qos.congestion_control(), CongestionControl::Block);
///
/// // High-priority real-time message with express delivery
/// let realtime_qos = QoS::default()
///     .with_priority(Priority::High)
///     .with_express(true)
///     .with_congestion_control(CongestionControl::Block);
///
/// // Best-effort background task that can be dropped under load
/// let background_qos = QoS::default()
///     .with_priority(Priority::Background)
///     .with_congestion_control(CongestionControl::Drop);
/// ```
// Internal layout:
// ```text
//  7 6 5 4 3 2 1 0
// +-+-+-+-+-+-+-+-+
// |ID |X|E|D| Prio|
// +---+-+-+-+-----+
// ```
//
// Where:
// - Bits 0-2: Priority (3 bits) - Message priority level
// - Bit 3: D (Droppable) - Congestion control flag (0=Block, 1=Drop)
// - Bit 4: E (Express) - Express delivery flag
// - Bit 5: X (Reserved) - Reserved for future use
// - Bits 6-7: ID - Used for message IDs, so QoS don't need to be re-encoded every time
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct QoS(u8);

impl QoS {
    /// Default QoS configuration: [`Priority::Medium`], Express=false,
    /// [`CongestionControl::Block`]
    pub const DEFAULT: Self = Self(Priority::DEFAULT as u8);

    const DROPPABLE: u8 = 1 << Priority::BITS;
    const EXPRESS: u8 = 1 << (Priority::BITS + 1);

    /// Creates a new QoS with default settings.
    ///
    /// Returns a QoS configured with:
    /// - Priority: Medium
    /// - Express: false
    /// - Congestion Control: Block
    pub const fn default() -> Self {
        Self::DEFAULT
    }

    /// Returns the priority level of this QoS.
    pub const fn priority(&self) -> Priority {
        unsafe { Priority::from_u8(self.0 & Priority::MASK) }
    }

    /// Sets the priority level and returns the modified QoS.
    ///
    /// # Example
    /// ```
    /// use thubo::{Priority, QoS};
    /// let qos = QoS::default().with_priority(Priority::High);
    /// ```
    pub const fn with_priority(mut self, priority: Priority) -> Self {
        self.0 &= !Priority::MASK;
        self.0 |= priority as u8;
        self
    }

    /// Returns whether express delivery is enabled.
    ///
    /// Express messages bypass certain queuing mechanisms for faster delivery.
    pub const fn express(&self) -> bool {
        self.0 & Self::EXPRESS != 0
    }

    /// Sets the express delivery flag and returns the modified QoS.
    ///
    /// # Example
    /// ```
    /// use thubo::QoS;
    /// let qos = QoS::default().with_express(true);
    /// ```
    pub const fn with_express(mut self, express: bool) -> Self {
        if express {
            self.0 |= Self::EXPRESS;
        } else {
            self.0 &= !Self::EXPRESS;
        }
        self
    }

    /// Returns the congestion control strategy.
    pub const fn congestion_control(&self) -> CongestionControl {
        unsafe { CongestionControl::from_bool(self.0 & Self::DROPPABLE == Self::DROPPABLE) }
    }

    /// Sets the congestion control strategy and returns the modified QoS.
    ///
    /// # Example
    /// ```
    /// use thubo::{CongestionControl, QoS};
    /// let qos = QoS::default().with_congestion_control(CongestionControl::Drop);
    /// ```
    pub const fn with_congestion_control(mut self, congestion_control: CongestionControl) -> Self {
        match congestion_control {
            CongestionControl::Block => self.0 &= !Self::DROPPABLE,
            CongestionControl::Drop => self.0 |= Self::DROPPABLE,
        }
        self
    }

    /// Returns the raw byte representation of this QoS.
    pub(crate) const fn as_u8(&self) -> u8 {
        self.0
    }

    /// Creates a QoS from a raw byte value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the byte value represents a valid QoS
    /// configuration.
    pub(crate) const unsafe fn from_u8(v: u8) -> Self {
        Self(v)
    }

    /// Generates a random QoS configuration for testing.
    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        QoS::default()
            .with_priority(Priority::rand())
            .with_congestion_control(CongestionControl::rand())
            .with_express(rand::random_bool(0.5))
    }
}

impl fmt::Debug for QoS {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QoS")
            .field("priority", &self.priority())
            .field("congestion_control", &self.congestion_control())
            .field("express", &self.express())
            .finish()
    }
}

impl Default for QoS {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qos_new() {
        let qos = QoS::default();
        assert_eq!(qos, QoS::DEFAULT);
        assert_eq!(qos, QoS::default());

        assert_eq!(qos.priority(), Priority::DEFAULT);
        assert_eq!(qos.congestion_control(), CongestionControl::DEFAULT);
        assert!(!qos.express());
    }

    #[test]
    fn test_priorities() {
        for priority in Priority::ALL {
            let qos = QoS::default().with_priority(priority);
            assert_eq!(qos.priority(), priority);
        }
    }

    #[test]
    fn test_express_flag() {
        let qos = QoS::default().with_express(true);
        assert!(qos.express());

        let qos = QoS::default().with_express(false);
        assert!(!qos.express());
    }

    #[test]
    fn test_congestion_control() {
        let qos = QoS::default().with_congestion_control(CongestionControl::Drop);
        assert_eq!(qos.congestion_control(), CongestionControl::Drop);

        let qos = QoS::default().with_congestion_control(CongestionControl::Block);
        assert_eq!(qos.congestion_control(), CongestionControl::Block);
    }

    #[test]
    fn test_combined_settings() {
        let qos = QoS::default()
            .with_priority(Priority::High)
            .with_express(true)
            .with_congestion_control(CongestionControl::Drop);

        assert_eq!(qos.priority(), Priority::High);
        assert!(qos.express());
        assert_eq!(qos.congestion_control(), CongestionControl::Drop);
    }

    #[test]
    fn test_as_u8_and_from_u8() {
        let qos = QoS::default()
            .with_priority(Priority::Low)
            .with_express(true)
            .with_congestion_control(CongestionControl::Drop);

        let byte = qos.as_u8();
        let restored = unsafe { QoS::from_u8(byte) };

        assert_eq!(qos, restored);
    }
}
