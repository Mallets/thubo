use std::sync::{
    LazyLock,
    atomic::{AtomicU16, Ordering},
};

pub(super) mod ringbuf;
pub(crate) mod rx;
pub(crate) mod tx;

pub(super) static LOCAL_EPOCH: LazyLock<quanta::Instant> = LazyLock::new(quanta::Instant::now);

// Status
#[repr(transparent)]
pub(crate) struct Status(AtomicU16);

pub(crate) mod status {
    use crate::Priority;

    const CONGESTED: u16 = 1 << 0;
    const FLUSH: u16 = 1 << Priority::NUM;
    pub(super) const DISABLED: u16 = FLUSH << Priority::NUM;
    pub(super) const BACKOFF: u16 = DISABLED << 1;

    pub(crate) const fn congested(p: Priority) -> u16 {
        CONGESTED << p as u16
    }

    pub(crate) const fn flush(p: Priority) -> u16 {
        FLUSH << p as u16
    }

    pub(crate) const fn any(v: u16, f: u16) -> bool {
        v & f != 0
    }
}

impl Status {
    const fn new() -> Self {
        Self(AtomicU16::new(0))
    }

    fn set(&self, f: u16) -> u16 {
        self.0.fetch_or(f, Ordering::AcqRel)
    }

    fn unset(&self, f: u16) -> u16 {
        self.0.fetch_and(!f, Ordering::AcqRel)
    }

    pub(crate) fn any(&self, f: u16) -> bool {
        status::any(self.0.load(Ordering::Acquire), f)
    }
}
