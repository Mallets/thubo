use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const fn duration_to_u64(duration: Duration) -> u64 {
    (duration.as_secs() << 32) | duration.subsec_nanos() as u64
}

const fn u64_to_duration(secs_nanos: u64) -> Duration {
    Duration::new(secs_nanos >> 32, secs_nanos as u32)
}

pub(crate) struct AtomicDuration(AtomicU64);

impl AtomicDuration {
    pub(crate) fn new(duration: Duration) -> Self {
        assert!(duration.as_secs() <= u32::MAX as u64);
        Self(AtomicU64::new(duration_to_u64(duration)))
    }

    pub(crate) fn store(&self, duration: Duration, order: Ordering) {
        assert!(duration.as_secs() <= u32::MAX as u64);
        self.0.store(duration_to_u64(duration), order);
    }

    pub(crate) fn load(&self, order: Ordering) -> Duration {
        u64_to_duration(self.0.load(order))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_duration() {
        // 1. Basic construction and load
        let duration = Duration::from_secs(10);
        let atomic = AtomicDuration::new(duration);
        assert_eq!(atomic.load(Ordering::Relaxed), duration);

        // 2. Store and load operations
        let new_duration = Duration::from_secs(20);
        atomic.store(new_duration, Ordering::Relaxed);
        assert_eq!(atomic.load(Ordering::Relaxed), new_duration);

        // 3. Duration with nanoseconds
        let precise_duration = Duration::new(42, 123_456_789);
        atomic.store(precise_duration, Ordering::SeqCst);
        assert_eq!(atomic.load(Ordering::SeqCst), precise_duration);

        // 4. Zero duration
        let zero = Duration::from_secs(0);
        atomic.store(zero, Ordering::Relaxed);
        assert_eq!(atomic.load(Ordering::Relaxed), zero);

        // 5. Maximum nanoseconds (just under 1 second)
        let max_nanos = Duration::new(5, 999_999_999);
        atomic.store(max_nanos, Ordering::Relaxed);
        assert_eq!(atomic.load(Ordering::Relaxed), max_nanos);

        // 6. Maximum valid duration (u32::MAX seconds)
        let max_valid = Duration::from_secs(u32::MAX as u64);
        atomic.store(max_valid, Ordering::Relaxed);
        assert_eq!(atomic.load(Ordering::Relaxed), max_valid);

        // 7. Various ordering modes
        let d = Duration::from_millis(500);
        atomic.store(d, Ordering::Release);
        assert_eq!(atomic.load(Ordering::Acquire), d);
    }

    #[test]
    fn test_duration_encoding_decoding() {
        // 1. Round-trip with seconds only
        let d1 = Duration::from_secs(100);
        let encoded = duration_to_u64(d1);
        let decoded = u64_to_duration(encoded);
        assert_eq!(d1, decoded);

        // 2. Round-trip with seconds and nanoseconds
        let d2 = Duration::new(3600, 500_000_000);
        let encoded = duration_to_u64(d2);
        let decoded = u64_to_duration(encoded);
        assert_eq!(d2, decoded);

        // 3. Round-trip with zero
        let d3 = Duration::from_secs(0);
        let encoded = duration_to_u64(d3);
        let decoded = u64_to_duration(encoded);
        assert_eq!(d3, decoded);
    }

    #[test]
    #[should_panic]
    fn test_new_panic_on_overflow() {
        // Attempting to create with duration exceeding u32::MAX seconds should panic
        let invalid = Duration::from_secs(u32::MAX as u64 + 1);
        let _ = AtomicDuration::new(invalid);
    }

    #[test]
    #[should_panic]
    fn test_store_panic_on_overflow() {
        // Store should panic when duration exceeds u32::MAX seconds
        let atomic = AtomicDuration::new(Duration::from_secs(0));
        let invalid = Duration::from_secs(u32::MAX as u64 + 1);
        atomic.store(invalid, Ordering::Relaxed);
    }
}
