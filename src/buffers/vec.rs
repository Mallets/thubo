use core::{mem, num::NonZeroUsize};

use super::{
    reader::HasReader,
    writer::{BacktrackableWriter, DidntWrite, HasWriter, Writer},
};

// Writer implementations
impl HasWriter for &mut Vec<u8> {
    type Writer = Self;

    fn writer(self) -> Self::Writer {
        self
    }
}

impl Writer for &mut Vec<u8> {
    fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite> {
        if bytes.is_empty() {
            return Err(DidntWrite);
        }
        self.extend_from_slice(bytes);
        // SAFETY: this operation is safe since we early return in case bytes is empty
        Ok(unsafe { NonZeroUsize::new_unchecked(bytes.len()) })
    }

    fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite> {
        self.write(bytes).map(|_| ())
    }

    fn write_u8(&mut self, byte: u8) -> Result<(), DidntWrite> {
        self.push(byte);
        Ok(())
    }

    fn remaining(&self) -> usize {
        usize::MAX
    }

    unsafe fn with_slot<F>(&mut self, mut len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        self.reserve(len);

        // SAFETY: we already reserved len elements on the vector.
        let s = super::unsafe_slice_mut!(self.spare_capacity_mut(), ..len);
        // SAFETY: converting MaybeUninit<u8> into [u8] is safe because we are going to write on it.
        // The returned len tells us how many bytes have been written so as to update the len accordingly.
        len = unsafe { write(&mut *(s as *mut [mem::MaybeUninit<u8>] as *mut [u8])) };
        // SAFETY: we already reserved len elements on the vector.
        unsafe { self.set_len(self.len() + len) };

        NonZeroUsize::new(len).ok_or(DidntWrite)
    }
}

impl BacktrackableWriter for &mut Vec<u8> {
    type Mark = usize;

    fn mark(&mut self) -> Self::Mark {
        self.len()
    }

    fn rewind(&mut self, mark: Self::Mark) -> bool {
        self.truncate(mark);
        true
    }
}

// Reader implementations
impl<'a> HasReader for &'a Vec<u8> {
    type Reader = &'a [u8];

    fn reader(self) -> Self::Reader {
        self
    }
}
