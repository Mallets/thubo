use core::fmt;
use std::{
    cmp,
    // sync::atomic::{self, AtomicU32},
};

use crate::protocol::FrameSn;

/// Mask for sequence numbers (28 bits - allows for 4 bytes max when encoded)
const MASK: FrameSn = (u32::MAX >> 4) as FrameSn;

/// Sequence number with wrapping arithmetic and comparison.
///
/// This type implements sequence number arithmetic as defined in RFC 1982,
/// handling wraparound correctly for a 28-bit sequence space.
/// The comparison logic properly handles the case where sequence numbers
/// wrap around from the maximum value back to zero.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeqNum(u32);

impl SeqNum {
    /// Creates a new sequence number from a u32 value.
    ///
    /// The value is masked to ensure it fits within the valid sequence number
    /// range.
    pub(crate) const fn new(value: u32) -> Self {
        let value = value & MASK;
        SeqNum(value)
    }

    /// Returns the raw u32 value of the sequence number.
    pub(crate) const fn get(&self) -> u32 {
        self.0
    }

    /// Sets this sequence number to the value of another sequence number.
    pub(crate) fn set(&mut self, sn: Self) {
        *self = sn;
    }

    /// Returns the next sequence number in the sequence.
    ///
    /// Handles wraparound correctly when the sequence number reaches the
    /// maximum value.
    pub(crate) fn next(&self) -> Self {
        Self(self.0.wrapping_add(1) & MASK)
    }

    /// Increments this sequence number and returns the new value.
    ///
    /// This method modifies the sequence number in place and returns the new
    /// value.
    pub(crate) fn increment(&mut self) -> Self {
        let next = self.next();
        self.set(next);
        next
    }

    /// Generates a random sequence number for testing.
    #[cfg(test)]
    pub(crate) fn rand() -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        Self::new(rng.random())
    }
}

impl PartialOrd for SeqNum {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SeqNum {
    /// Compares sequence numbers using serial number arithmetic.
    ///
    /// This implementation correctly handles wraparound by treating the
    /// sequence space as circular. A sequence number is considered "less
    /// than" another if it is within half the sequence space ahead of it.
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if self == other {
            return cmp::Ordering::Equal;
        }

        let gap = other.0.wrapping_sub(self.0) & MASK;
        if (gap != 0) && ((gap & !(MASK >> 1)) == 0) {
            cmp::Ordering::Less
        } else {
            cmp::Ordering::Greater
        }
    }
}

impl fmt::Display for SeqNum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}", self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seq_num_vle() {
        assert_eq!(crate::codec::core::vle::vle_len(super::MASK as u64), 4);
    }

    #[test]
    fn test_new_and_get() {
        // Test within mask range
        let sn = SeqNum::new(5);
        assert_eq!(sn.get(), 5);

        // Test masking - values larger than MASK get masked
        let sn = SeqNum::new(42);
        assert_eq!(sn.get(), 42 & MASK); // 42 & 7 = 2

        let large_value = u32::MAX;
        let sn = SeqNum::new(large_value);
        assert_eq!(sn.get(), large_value & MASK); // Should be 7
    }

    #[test]
    fn test_set() {
        let mut sn = SeqNum::new(2);
        let new_sn = SeqNum::new(4);
        sn.set(new_sn);
        assert_eq!(sn.get(), 4);
    }

    #[test]
    fn test_next() {
        let sn = SeqNum::new(3);
        let next = sn.next();
        assert_eq!(next.get(), 4);

        // Test wraparound at MASK
        let max_sn = SeqNum::new(MASK);
        let next = max_sn.next();
        assert_eq!(next.get(), 0);
    }

    #[test]
    fn test_increment() {
        let mut sn = SeqNum::new(2);
        let result = sn.increment();
        assert_eq!(sn.get(), 3);
        assert_eq!(result.get(), 3);

        // Test wraparound
        let mut sn = SeqNum::new(MASK);
        let result = sn.increment();
        assert_eq!(sn.get(), 0);
        assert_eq!(result.get(), 0);
    }

    #[test]
    fn test_ordering() {
        let sn1 = SeqNum::new(1);
        let sn2 = SeqNum::new(2);
        let sn3 = SeqNum::new(3);

        assert!(sn1 < sn2);
        assert!(sn2 < sn3);
        assert!(sn1 < sn3);
        assert!(sn2 > sn1);
        assert!(sn3 > sn1);
    }

    #[test]
    fn test_ordering_wraparound() {
        // With MASK=7, sequence space is 0-7
        // Half space is 3-4
        let sn_low = SeqNum::new(MASK);
        let sn_high = SeqNum::new(6);

        // sn_high is more than half space away, so it's "less" due to wraparound
        assert!(sn_high > sn_low);
        assert!(sn_low < sn_high);
    }

    #[test]
    fn test_ordering_half_space() {
        // Test the boundary of half the sequence space
        // The sequence space is 0..=MASK (MASK+1 values)
        // Half of the sequence space is (MASK+1)/2
        // The comparison logic: if the gap from self to other is < half space,
        // then self < other, otherwise it's a wraparound case
        let half_space = (MASK >> 1) + 1; // This is (MASK+1)/2

        let sn0 = SeqNum::new(0);
        let sn1 = SeqNum::new(1);
        let sn_before_half = SeqNum::new(half_space - 1);
        let sn_at_half = SeqNum::new(half_space);
        let sn_after_half = SeqNum::new(half_space + 1);
        let sn_near_end = SeqNum::new(MASK - 1);
        let sn_max = SeqNum::new(MASK);

        // Normal ordering within half space
        assert!(sn0 < sn1);
        assert!(sn0 < sn_before_half);

        // At exactly half the sequence space, the comparison depends on
        // the implementation. Looking at the cmp function:
        // gap = half_space - 0 = half_space
        // (gap & !(MASK >> 1)) checks if gap > MASK >> 1
        // Since gap == half_space and half_space == (MASK >> 1) + 1,
        // this will be non-zero, so sn0 > sn_at_half (wraparound interpretation)
        assert!(sn0 > sn_at_half);

        // Values more than half away are considered "less" due to wraparound
        assert!(sn_after_half < sn0); // after_half is "before" 0 in circular space
        assert!(sn_near_end < sn0); // near_end is "before" 0 in circular space
        assert!(sn_max < sn0); // max is "before" 0 in circular space
    }

    #[test]
    fn test_partial_ord() {
        let sn1 = SeqNum::new(2);
        let sn2 = SeqNum::new(4);

        assert_eq!(sn1.partial_cmp(&sn2), Some(cmp::Ordering::Less));
        assert_eq!(sn2.partial_cmp(&sn1), Some(cmp::Ordering::Greater));
        assert_eq!(sn1.partial_cmp(&sn1), Some(cmp::Ordering::Equal));
    }

    #[test]
    fn test_debug() {
        let sn = SeqNum::new(5);
        let debug_str = format!("{:?}", sn);
        assert!(debug_str.contains("5"));
    }
}
