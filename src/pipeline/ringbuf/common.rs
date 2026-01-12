use std::{
    mem::{self, MaybeUninit},
    ops::{Deref, DerefMut},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use crossbeam_utils::CachePadded;

use crate::protocol::Priority;

pub(super) mod update {
    use super::{AtomicIdx, Idx, Ordering, Priority};

    pub(crate) fn s(aidx: &AtomicIdx, _priority: Priority, idx: Idx) {
        aidx.store(idx, Ordering::Release);
    }

    pub(crate) fn m(aidx: &AtomicIdx, priority: Priority, idx: Idx) {
        aidx.update(priority, idx, Ordering::Release, Ordering::Acquire);
    }
}

mod flag {
    use super::{Idx, Priority};

    pub(crate) const BOOKED: u8 = 1 << 0;

    pub(super) const fn prio(p: Priority, flag: u8) -> u64 {
        (flag as u64) << (8 + Idx::shift(p))
    }
}

// -- non atomic

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub(super) struct Idx(u64);

impl Idx {
    const MASK: u64 = 0xFFFF;
    const BITS: u32 = Self::MASK.trailing_ones();

    const fn new() -> Self {
        Self(0)
    }

    #[must_use]
    const fn mask(priority: Priority) -> u64 {
        Self::MASK << Self::shift(priority)
    }

    #[must_use]
    const fn shift(priority: Priority) -> u32 {
        Self::BITS * (priority as u32)
    }

    const fn index(&self, priority: Priority) -> u8 {
        (self.0 >> Self::shift(priority)) as u8
    }

    const fn increment(&mut self, priority: Priority) {
        let shift = Self::shift(priority);
        let mask = (u8::MAX as u64) << shift;

        let v = (self.index(priority).wrapping_add(1) as u64) << shift;
        self.0 = (self.0 & !mask) | v;
    }

    const fn any(&self, flag: u64) -> bool {
        self.0 & flag != 0
    }

    const fn set(&mut self, flag: u64) {
        self.0 |= flag;
    }

    const fn unset(&mut self, flag: u64) {
        self.0 &= !flag;
    }
}

// -- atomic
#[repr(transparent)]
pub(super) struct AtomicIdx(AtomicU64);

impl AtomicIdx {
    const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    fn load(&self, order: Ordering) -> Idx {
        Idx(self.0.load(order))
    }

    fn store(&self, idx: Idx, order: Ordering) {
        self.0.store(idx.0, order);
    }

    fn update(&self, priority: Priority, idx: Idx, success: Ordering, failure: Ordering) {
        let mask = Idx::mask(priority);
        let tmp = idx.0 & mask;

        let mut old = self.0.load(Ordering::Acquire);
        loop {
            let new = (old & !mask) | tmp;
            match self.0.compare_exchange_weak(old, new, success, failure) {
                Ok(_) => break,
                Err(new) => old = new,
            }
        }
    }

    fn set(&self, flag: u64, order: Ordering) -> Idx {
        Idx(self.0.fetch_or(flag, order))
    }

    // fn unset(&self, flag: u64, order: Ordering) -> Idx {
    //     Idx(self.0.fetch_and(!flag, order))
    // }
}

/// Internal ringbuffer storage. This type is private to the crate.
///
/// It stores the raw boxed slice pointer and the atomic indices used for
/// synchronization. The implementation uses monotonically increasing indices
/// (wrapping on overflow) and a power-of-two mask to convert indices to
/// positions inside the buffer.
pub(super) struct RingBuffer<T> {
    ptr: [*mut [MaybeUninit<T>]; Priority::NUM],
    mask: u8,
    idx_r: CachePadded<AtomicIdx>,
    idx_w: CachePadded<AtomicIdx>,
}

impl<T> RingBuffer<T> {
    pub(super) fn new(capacity: usize) -> Self {
        assert!(capacity.is_power_of_two(), "Capacity must be a power of 2");
        let max = 2_usize.pow(u8::BITS - 1);
        assert!(capacity <= max, "Max capacity is {max}");

        macro_rules! buf {
            () => {
                Box::into_raw(
                    (0..capacity)
                        .map(|_| MaybeUninit::uninit())
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                )
            };
        }

        // Inner container
        let ptr: [*mut [MaybeUninit<_>]; Priority::NUM] = [buf!(), buf!(), buf!(), buf!()];

        RingBuffer {
            // Keep the pointer to the boxed slice
            ptr,
            // Since capacity is a power of two, capacity-1 is a mask covering N elements overflowing when N elements
            // have been added. Indexes are left growing indefinitely and naturally wrap around once the
            // index increment reaches usize::MAX.
            mask: (capacity - 1) as u8,
            idx_r: CachePadded::new(AtomicIdx::new()),
            idx_w: CachePadded::new(AtomicIdx::new()),
        }
    }

    #[inline]
    const fn is_empty(r: Idx, w: Idx, p: Priority) -> bool {
        r.index(p) == w.index(p)
    }

    #[inline]
    const fn is_full(r: Idx, w: Idx, p: Priority, c: u8) -> bool {
        w.index(p).wrapping_sub(r.index(p)) == c
    }

    fn capacity(&self) -> u8 {
        unsafe { self.ptr.get_unchecked(0) }.len() as u8
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get_slice_mut(&self, priority: Priority) -> &mut [MaybeUninit<T>] {
        unsafe { &mut **self.ptr.get_unchecked(priority as usize) }
    }

    #[allow(clippy::mut_from_ref)]
    unsafe fn get_elem_mut(&self, priority: Priority, idx: Idx) -> &mut MaybeUninit<T> {
        let idx = idx.index(priority) & self.mask;
        unsafe { self.get_slice_mut(priority).get_unchecked_mut(idx as usize) }
    }
}

// The internal `RingBuffer` is stored inside an `Arc` and will be deallocated
// when the last writer or reader handle is dropped (i.e., when the `Arc`
// reference count reaches zero).
impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        let mut idx_r = self.idx_r.load(Ordering::Acquire);
        let idx_w = self.idx_w.load(Ordering::Acquire);

        for p in Priority::ALL {
            while idx_r.index(p) != idx_w.index(p) {
                // SAFETY: we are in Drop and we must clean up any elements still
                // present in the buffer. Since only one producer and one
                // consumer exist, and we're dropping the entire buffer, it is
                // safe to assume we can take ownership of remaining initialized
                // elements between `idx_r` and `idx_w`.
                let t = unsafe { mem::replace(self.get_elem_mut(p, idx_r), MaybeUninit::uninit()).assume_init() };
                mem::drop(t);
                idx_r.increment(p);
            }

            // At this point we've taken ownership of and dropped every
            // initialized element that was still present in the buffer. It is
            // important to drop all elements before freeing the backing storage
            // so that each element's destructor runs exactly once. Converting
            // the raw pointer back into a `Box` will free the allocation for
            // the slice itself.
            let ptr = unsafe { Box::from_raw(self.get_slice_mut(p)) };
            mem::drop(ptr);
        }
    }
}

/// Writer handle of the ringbuffer.
pub(super) struct RingBufferWriter<T> {
    inner: Arc<RingBuffer<T>>,
    cached_idx_r: Idx,
    local_idx_w: Idx,
    update: fn(&AtomicIdx, Priority, Idx),
}

unsafe impl<T: Send> Send for RingBufferWriter<T> {}
unsafe impl<T: Sync> Sync for RingBufferWriter<T> {}

impl<T> RingBufferWriter<T> {
    pub(super) fn new(inner: Arc<RingBuffer<T>>, update: fn(&AtomicIdx, Priority, Idx)) -> Self {
        Self {
            inner,
            cached_idx_r: Idx::new(),
            local_idx_w: Idx::new(),
            update,
        }
    }

    #[inline]
    fn is_full(&mut self, p: Priority) -> bool {
        let mut is_full = RingBuffer::<T>::is_full(self.cached_idx_r, self.local_idx_w, p, self.inner.capacity());
        if is_full {
            self.cached_idx_r = self.inner.idx_r.load(Ordering::Acquire);
            is_full = RingBuffer::<T>::is_full(self.cached_idx_r, self.local_idx_w, p, self.inner.capacity());
        }
        is_full
    }

    /// Push an element into the RingBuffer.
    ///
    /// Returns `Some(T)` when the buffer is full (giving back ownership of the value), otherwise returns `None` on
    /// success.
    #[inline]
    pub(super) fn push(&mut self, p: Priority, t: T) -> Option<T> {
        // Check if the ringbuffer is full.
        if self.is_full(p) {
            return Some(t);
        }

        // Insert the element in the ringbuffer
        let _ = mem::replace(
            unsafe { self.inner.get_elem_mut(p, self.local_idx_w) },
            MaybeUninit::new(t),
        );

        // Let's increment the counter and let it grow indefinitely and potentially overflow resetting it to 0.
        self.local_idx_w.increment(p);
        (self.update)(&self.inner.idx_w, p, self.local_idx_w);

        None
    }

    pub(super) fn book(&mut self, p: Priority, t: T) -> Result<WriterPeek<'_, T>, T> {
        let flag = flag::prio(p, flag::BOOKED);
        let is_booked = self.local_idx_w.any(flag);
        if is_booked {
            return Err(t);
        }

        if self.is_full(p) {
            return Err(t);
        }

        let _ = mem::replace(
            unsafe { self.inner.get_elem_mut(p, self.local_idx_w) },
            MaybeUninit::new(t),
        );

        let flag = flag::prio(p, flag::BOOKED);
        self.local_idx_w.set(flag);
        self.inner.idx_w.set(flag, Ordering::AcqRel);

        let wp = WriterPeek {
            writer: self,
            priority: p,
        };
        Ok(wp)
    }

    pub(super) fn peek(&mut self, p: Priority) -> Option<WriterPeek<'_, T>> {
        let flag = flag::prio(p, flag::BOOKED);
        if self.local_idx_w.any(flag) {
            let wp = WriterPeek {
                writer: self,
                priority: p,
            };
            return Some(wp);
        }
        None
    }
}

pub(crate) struct WriterPeek<'a, T> {
    writer: &'a mut RingBufferWriter<T>,
    priority: Priority,
}

impl<T> WriterPeek<'_, T> {
    pub(crate) fn commit(self) {
        let flag = flag::prio(self.priority, flag::BOOKED);
        self.writer.local_idx_w.unset(flag);
        self.writer.local_idx_w.increment(self.priority);
        (self.writer.update)(&self.writer.inner.idx_w, self.priority, self.writer.local_idx_w);
    }
}

impl<T> Deref for WriterPeek<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe {
            self.writer
                .inner
                .get_elem_mut(self.priority, self.writer.local_idx_w)
                .assume_init_ref()
        }
    }
}

impl<T> DerefMut for WriterPeek<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            self.writer
                .inner
                .get_elem_mut(self.priority, self.writer.local_idx_w)
                .assume_init_mut()
        }
    }
}

/// Reader handle of the ringbuffer.
pub(super) struct RingBufferReader<T> {
    inner: Arc<RingBuffer<T>>,
    local_idx_r: Idx,
    cached_idx_w: Idx,
    update: fn(&AtomicIdx, Priority, Idx),
}

unsafe impl<T: Send> Send for RingBufferReader<T> {}
unsafe impl<T: Sync> Sync for RingBufferReader<T> {}

impl<T> RingBufferReader<T> {
    pub(super) fn new(inner: Arc<RingBuffer<T>>, update: fn(&AtomicIdx, Priority, Idx)) -> Self {
        Self {
            inner,
            local_idx_r: Idx::new(),
            cached_idx_w: Idx::new(),
            update,
        }
    }

    #[inline]
    fn is_empty(&mut self, p: Priority) -> bool {
        let mut is_empty = RingBuffer::<T>::is_empty(self.local_idx_r, self.cached_idx_w, p);
        if is_empty {
            self.cached_idx_w = self.inner.idx_w.load(Ordering::Acquire);
            is_empty = RingBuffer::<T>::is_empty(self.local_idx_r, self.cached_idx_w, p);
        }
        is_empty
    }

    #[inline]
    fn pull_priority_inner(&mut self, p: Priority) -> Option<T> {
        // Check if the ringbuffer is potentially empty
        if !self.is_empty(p) {
            // Remove the element from the ringbuffer
            let t = unsafe {
                mem::replace(self.inner.get_elem_mut(p, self.local_idx_r), MaybeUninit::uninit()).assume_init()
            };
            // Let's increment the counter and let it grow indefinitely
            // and potentially overflow resetting it to 0.
            self.local_idx_r.increment(p);
            (self.update)(&self.inner.idx_r, p, self.local_idx_r);

            return Some(t);
        }

        None
    }

    #[cfg(test)]
    fn peek_priority_inner(&mut self, p: Priority) -> Option<&T> {
        if !self.is_empty(p) {
            let t = unsafe { self.inner.get_elem_mut(p, self.local_idx_r).assume_init_ref() };
            return Some(t);
        }
        None
    }

    /// Pull an element from the ringbuffer.
    ///
    /// Returns `Some(T)` if an element is available, otherwise `None` when the buffer is empty.
    #[inline]
    pub(super) fn pull(&mut self) -> Option<T> {
        for p in Priority::ALL {
            let res = self.pull_priority_inner(p);
            if res.is_some() {
                return res;
            }
        }
        None
    }

    #[inline]
    pub(super) fn pull_priority(&mut self, p: Priority) -> Pull<T> {
        if let Some(t) = self.pull_priority_inner(p) {
            return Pull::Some(t);
        }

        let flag = flag::prio(p, flag::BOOKED);
        if self.cached_idx_w.any(flag) {
            Pull::Booked
        } else {
            Pull::None
        }
    }

    /// Peek an element from the ringbuffer without pulling it out.
    ///
    /// Returns `Some(&T)` when at lease one element is present, or `None` when the buffer is empty.
    #[inline]
    #[cfg(test)]
    pub(super) fn peek(&mut self) -> Option<&T> {
        for p in Priority::ALL {
            if !self.is_empty(p) {
                let t = unsafe { self.inner.get_elem_mut(p, self.local_idx_r).assume_init_ref() };
                return Some(t);
            }
        }
        None
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn peek_priority(&mut self, p: Priority) -> Option<&T> {
        self.peek_priority_inner(p)
    }

    #[cfg(test)]
    #[inline]
    pub(super) fn peek_priority_mut(&mut self, p: Priority) -> Option<&mut T> {
        if !self.is_empty(p) {
            let t = unsafe { self.inner.get_elem_mut(p, self.local_idx_r).assume_init_mut() };
            return Some(t);
        }
        None
    }

    /// Peek a mutable element from the ringbuffer without pulling it out.
    ///
    /// Returns `Some(&mut T)` when at lease one element is present, or `None` when the buffer is empty.
    #[inline]
    pub(super) fn peek_mut(&mut self) -> Option<&mut T> {
        for p in Priority::ALL {
            if !self.is_empty(p) {
                let t = unsafe { self.inner.get_elem_mut(p, self.local_idx_r).assume_init_mut() };
                return Some(t);
            }
        }
        None
    }
}

pub(crate) enum Pull<T> {
    Some(T),
    Booked,
    None,
}
