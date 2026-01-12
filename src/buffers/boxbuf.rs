use core::{fmt, num::NonZeroUsize};

use super::{
    reader::HasReader,
    writer::{BacktrackableWriter, DidntWrite, HasWriter, Writer},
};

/// A fixed-capacity buffer that tracks both capacity and current length.
///
/// `BoxBuf` wraps a `Box<[u8]>` and maintains a separate length field to track
/// how much data has been written. This allows efficient reuse by calling
/// [`clear()`](Self::clear) without reallocating.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct BoxBuf {
    buffer: Box<[u8]>,
    len: usize,
}

impl BoxBuf {
    /// Creates a buffer with the specified capacity.
    ///
    /// The buffer is pre-allocated with `capacity` bytes but has zero length.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: vec![0u8; capacity].into_boxed_slice(),
            len: 0,
        }
    }

    /// Returns the total capacity of the buffer.
    ///
    /// This is the maximum number of bytes the buffer can hold.
    pub(crate) const fn capacity(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the total length of the buffer.
    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    /// Returns a slice containing the written data.
    pub(crate) fn as_slice(&self) -> &[u8] {
        // SAFETY: self.len is ensured by the writer to be smaller than buffer length.
        super::unsafe_slice!(self.buffer, ..self.len)
    }

    /// Returns a mutable slice containing the written data.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: self.len is ensured by the writer to be smaller than buffer length.
        super::unsafe_slice_mut!(self.buffer, ..self.len)
    }

    /// Clears the buffer, resetting its length to zero.
    ///
    /// This does not deallocate or modify the underlying capacity.
    pub(crate) fn clear(&mut self) {
        self.len = 0;
    }

    fn as_writable_slice(&mut self) -> &mut [u8] {
        // SAFETY: self.len is ensured by the writer to be smaller than buffer length.
        super::unsafe_slice_mut!(self.buffer, self.len..)
    }
}

impl fmt::Debug for BoxBuf {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:02x?}", self.as_slice())
    }
}

// Writer implementations

/// Implements [`HasWriter`] for mutable reference to `BoxBuf`.
impl HasWriter for &mut BoxBuf {
    type Writer = Self;

    fn writer(self) -> Self::Writer {
        self
    }
}

/// Implements [`Writer`] for mutable reference to `BoxBuf`.
///
/// Unlike `Vec<u8>`, writes to `BoxBuf` can fail if the capacity is exceeded.
/// The buffer maintains a fixed capacity and will not reallocate.
impl Writer for &mut BoxBuf {
    fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite> {
        let mut writer = self.as_writable_slice().writer();
        let len = writer.write(bytes)?;
        self.len += len.get();
        Ok(len)
    }

    fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite> {
        let mut writer = self.as_writable_slice().writer();
        writer.write_exact(bytes)?;
        self.len += bytes.len();
        Ok(())
    }

    fn remaining(&self) -> usize {
        self.capacity() - self.len()
    }

    unsafe fn with_slot<F>(&mut self, len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        if self.remaining() < len {
            return Err(DidntWrite);
        }

        // SAFETY: self.remaining() >= len
        let written = write(unsafe { self.as_writable_slice().get_unchecked_mut(..len) });
        self.len += written;

        NonZeroUsize::new(written).ok_or(DidntWrite)
    }
}

/// Implements [`BacktrackableWriter`] for mutable reference to `BoxBuf`.
///
/// This allows marking positions and rewinding to them without deallocation.
impl BacktrackableWriter for &mut BoxBuf {
    type Mark = usize;

    fn mark(&mut self) -> Self::Mark {
        self.len
    }

    fn rewind(&mut self, mark: Self::Mark) -> bool {
        self.len = mark;
        true
    }
}

/// Implements [`std::io::Write`] for mutable reference to `BoxBuf`.
///
/// This allows `BoxBuf` to be used with standard library I/O traits.
impl std::io::Write for &mut BoxBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match <Self as Writer>::write(self, buf) {
            Ok(n) => Ok(n.get()),
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "UnexpectedEof")),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Reader implementations

/// Implements [`HasReader`] for shared reference to `BoxBuf`.
impl<'a> HasReader for &'a BoxBuf {
    type Reader = &'a [u8];

    fn reader(self) -> Self::Reader {
        self.as_slice()
    }
}
