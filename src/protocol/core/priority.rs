use core::{fmt, hash::Hash};
use std::{
    cmp::Ordering,
    ops::{Index, IndexMut},
};

/// Message priority levels for [QoS](`super::QoS`).
///
/// Priorities are ordered from highest ([`Priority::High`]) to lowest
/// ([`Priority::Background`]), with lower numeric values indicating higher
/// priority.
///
/// This is a **strict priority** scheduler: higher priority messages are always
/// processed before lower priority messages. This means lower priority levels
/// may experience starvation if higher priority traffic is sustained.
#[repr(u8)]
#[derive(Debug, Copy, Clone, Eq, Hash, PartialEq)]
pub enum Priority {
    /// Highest priority - Reserved for critical control and time-sensitive messages.
    ///
    /// **Use sparingly.** This priority level should be reserved for messages requiring
    /// the lowest possible latency and highest delivery priority, such as critical control
    /// messages or time-sensitive data that must be delivered immediately.
    ///
    /// **Warning:** Excessive use will starve all lower priority traffic. If sustained High
    /// priority traffic exceeds available bandwidth, even Medium priority messages will be
    /// delayed indefinitely.
    ///
    /// **Typical usage:** Protocol control messages, real-time sensor data, timing-critical events.
    ///
    /// **Examples:** Protocol negotiation, emergency shutdown, game state updates (30-60 Hz),
    /// IoT sensor data, trading signals.
    High = 0, // 0b000

    /// Medium priority - For interactive operations requiring responsive delivery.
    ///
    /// Use for user-facing operations and request/response patterns where responsiveness
    /// is important to user experience, but can tolerate slightly higher latency than High
    /// priority traffic.
    ///
    /// **Warning:** High-volume Medium priority traffic will starve Low and Background priorities.
    /// Monitor traffic distribution to maintain overall system responsiveness.
    ///
    /// **Typical usage:** User-initiated operations, API requests, interactive UI updates.
    ///
    /// **Examples:** Button clicks, search queries, form submissions, drag-and-drop operations.
    Medium = 1, // 0b001

    /// Low priority - For background operations and bulk data transfers.
    ///
    /// Use for operations that are important but not time-sensitive, such as prefetching,
    /// background synchronization, or bulk data transfers. These messages will be delivered
    /// when higher priority queues are empty or below their fair share.
    ///
    /// **Typical usage:** Prefetch operations, background sync, secondary UI updates.
    ///
    /// **Examples:** Lazy-loaded content, autocomplete suggestions, cache warming.
    Low = 2, // 0b010

    /// Lowest priority - For non-critical background tasks.
    ///
    /// Use for operations that have no urgency and can tolerate significant delays, such as
    /// analytics, logging, or maintenance tasks. These messages are delivered only when all
    /// higher priority queues are empty.
    ///
    /// **Typical usage:** Analytics, logging, telemetry, maintenance operations.
    ///
    /// **Examples:** Usage metrics, debug logs, periodic health checks, cleanup tasks.
    Background = 3, /* 0b011 */
}

impl Default for Priority {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl<T> Index<Priority> for [T; Priority::NUM] {
    type Output = T;

    fn index(&self, index: Priority) -> &Self::Output {
        // SAFETY: Priority enum has exactly NUM variants (0..=3), and the array
        // has exactly NUM elements. All Priority values are guaranteed to be
        // valid indices by the type system.
        unsafe { self.get_unchecked(index as usize) }
    }
}

impl<T> IndexMut<Priority> for [T; Priority::NUM] {
    fn index_mut(&mut self, index: Priority) -> &mut Self::Output {
        // SAFETY: Priority enum has exactly NUM variants (0..=3), and the array
        // has exactly NUM elements. All Priority values are guaranteed to be
        // valid indices by the type system.
        unsafe { self.get_unchecked_mut(index as usize) }
    }
}

impl Priority {
    /// Default priority level (Data)
    pub(crate) const DEFAULT: Self = Self::Medium;

    /// Number of bits used to encode priority (2 bits = 4 priority levels)
    pub(crate) const BITS: u32 = 2;

    /// The bitmask for extracting priority bits (0b11 = 2 bits)
    pub(crate) const MASK: u8 = 0b11;

    /// The lowest priority level (Background)
    pub(crate) const MIN: Self = Self::Background;

    /// The highest priority level (High)
    pub(crate) const MAX: Self = Self::High;

    /// The total number of available priority levels (4)
    pub(crate) const NUM: usize = 1 + Self::MIN as usize - Self::MAX as usize;

    #[cfg(test)]
    /// Range of valid priority values (0..=3)
    pub(crate) const RANGE: core::ops::RangeInclusive<u8> = (Priority::MAX as u8..=Priority::MIN as u8);

    // #[cfg(test)]
    /// Array containing all priority levels in order from highest to lowest
    pub(crate) const ALL: [Priority; Priority::NUM] =
        [Priority::High, Priority::Medium, Priority::Low, Priority::Background];

    // Compile-time assertion to ensure mask covers all priority values
    const _CHECK: () = {
        assert!(Self::MIN as u8 == 0);
        assert!(Self::MAX as u8 == Self::MASK);
    };

    /// Creates a Priority from a raw u8 value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `v` is a valid Priority value (0..=7).
    /// Values outside this range will result in undefined behavior.
    pub(crate) const unsafe fn from_u8(v: u8) -> Priority {
        unsafe { core::mem::transmute(v) }
    }
}

impl Priority {
    /// Returns the string representation of the priority level.
    pub const fn as_str(self) -> &'static str {
        match self {
            Priority::High => "High",
            Priority::Medium => "Medium",
            Priority::Low => "Low",
            Priority::Background => "Background",
        }
    }

    /// Generates a random priority value for testing.
    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        use rand::Rng;

        let mut rng = rand::rng();
        let value: u8 = rng.random_range(Priority::RANGE);
        unsafe { Priority::from_u8(value) }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher value means lower priority, we inverse the order
        let s = *self as u8;
        let o = *other as u8;
        (o).cmp(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_u8() {
        // Test that converting to u8 and back works correctly for all priorities
        for priority in Priority::ALL {
            let as_u8 = priority as u8;
            let restored = unsafe { Priority::from_u8(as_u8) };
            assert_eq!(priority, restored);
        }
    }

    #[test]
    fn test_priority_ordering() {
        // Test that priorities have correct numeric ordering
        assert!(Priority::High > Priority::Medium);
        assert!(Priority::Medium > Priority::Low);
        assert!(Priority::Low > Priority::Background);

        Priority::ALL.iter().fold(None, |prev, p| {
            if let Some(prev) = prev {
                assert!(prev > *p);
            }
            Some(*p)
        });
    }
}
