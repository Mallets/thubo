mod boxbuf;
pub(crate) mod bytes;
pub(crate) mod chunk;
mod slice;
mod vec;

pub(crate) use boxbuf::*;
pub(crate) use bytes::*;
pub(crate) use chunk::*;

// SAFETY: this crate operates on eventually initialized slices for read and write.
// Because of that, internal buffers implementation keeps track of various slices indexes.
// Boundaries checks are performed by individual implementations every time they need to access a slices.
// This means, that accessing a slice with [<range>] syntax after having already boundaries.
// In case of access violation the program will panic.
// However, it is desirable to avoid redundant checks for performance reasons.
// Nevertheless, it is desirable to keep those checks for testing and debugging purposes.
// Hence, the macros below will allow to switch boundaries check in case of test and to avoid them in all the other
// cases.
#[cfg(test)]
macro_rules! unsafe_slice {
    ($s:expr,$r:expr) => {
        &$s[$r]
    };
}

#[cfg(test)]
macro_rules! unsafe_slice_mut {
    ($s:expr,$r:expr) => {
        &mut $s[$r]
    };
}

#[cfg(not(test))]
macro_rules! unsafe_slice {
    ($s:expr,$r:expr) => {{
        let slice = &*$s;
        let index = $r;
        unsafe { slice.get_unchecked(index) }
    }};
}

#[cfg(not(test))]
macro_rules! unsafe_slice_mut {
    ($s:expr,$r:expr) => {{
        let slice = &mut *$s;
        let index = $r;
        unsafe { slice.get_unchecked_mut(index) }
    }};
}

pub(crate) use unsafe_slice;
pub(crate) use unsafe_slice_mut;

/// Traits for writing data to buffers.
pub(crate) mod writer {
    use core::num::NonZeroUsize;

    use super::Chunk;

    /// Error type indicating a write operation failed.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct DidntWrite;

    /// A trait for writing bytes into a buffer.
    ///
    /// This trait provides methods for writing data into buffers, with support
    /// for both partial writes and exact-length writes.
    pub(crate) trait Writer {
        /// Writes as many bytes as possible from `bytes` into the buffer.
        ///
        /// Returns the number of bytes actually written, which may be less than
        /// the length of `bytes` if the buffer has insufficient space.
        ///
        /// # Errors
        ///
        /// Returns `DidntWrite` if no bytes could be written.
        fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite>;

        /// Writes all bytes from `bytes` into the buffer.
        ///
        /// # Errors
        ///
        /// Returns `DidntWrite` if the buffer has insufficient space to write
        /// all bytes.
        fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite>;

        /// Returns the number of bytes that can still be written to this
        /// buffer.
        fn remaining(&self) -> usize;

        /// Writes a single byte to the buffer.
        ///
        /// # Errors
        ///
        /// Returns `DidntWrite` if the buffer is full.
        fn write_u8(&mut self, byte: u8) -> Result<(), DidntWrite> {
            self.write_exact(core::slice::from_ref(&byte))
        }

        /// Writes the contents of an `Chunk` to the buffer.
        ///
        /// # Errors
        ///
        /// Returns `DidntWrite` if the buffer has insufficient space.
        fn write_chunk(&mut self, slice: &Chunk) -> Result<(), DidntWrite> {
            self.write_exact(slice.as_slice())
        }

        /// Returns `true` if the buffer has space for more data.
        #[allow(unused)]
        fn can_write(&self) -> bool {
            self.remaining() != 0
        }

        /// Provides a buffer of exactly `len` uninitialized bytes to `write` to
        /// allow in-place writing.
        ///
        /// This method allows writing directly into the buffer's internal
        /// storage without copying. The closure `write` receives a
        /// mutable slice and must return the number of bytes it
        /// actually wrote.
        ///
        /// # Safety
        ///
        /// Caller must ensure that `write` returns an integer less than or
        /// equal to the length of the slice passed as argument.
        unsafe fn with_slot<F>(&mut self, len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
        where
            F: FnOnce(&mut [u8]) -> usize;
    }

    /// A trait for writers that support marking positions and rewinding.
    ///
    /// This allows saving a position in the buffer and later returning to it,
    /// effectively undoing writes that occurred in between.
    pub(crate) trait BacktrackableWriter: Writer {
        /// The type used to represent a marked position.
        type Mark;

        /// Marks the current position in the buffer.
        fn mark(&mut self) -> Self::Mark;

        /// Rewinds the buffer to a previously marked position.
        ///
        /// Returns `true` if the rewind was successful, `false` otherwise.
        fn rewind(&mut self, mark: Self::Mark) -> bool;
    }

    /// A trait for types that can provide a writer.
    pub(crate) trait HasWriter {
        /// The type of writer this type provides.
        type Writer: Writer;

        /// Returns the most appropriate writer for `self`.
        fn writer(self) -> Self::Writer;
    }
}

/// Traits for reading data from buffers.
pub(crate) mod reader {
    use core::num::NonZeroUsize;

    use super::Chunk;

    /// Error type indicating a read operation failed.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct DidntRead;

    /// A trait for reading bytes from a buffer.
    ///
    /// This trait provides methods for reading data from buffers, with support
    /// for both partial reads and exact-length reads. It also supports
    /// zero-copy reads via `Chunk` when possible.
    pub(crate) trait Reader {
        /// Reads as many bytes as possible into `into`.
        ///
        /// Returns the number of bytes actually read, which may be less than
        /// the length of `into` if the buffer has insufficient data.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if no bytes could be read.
        fn read(&mut self, into: &mut [u8]) -> Result<NonZeroUsize, DidntRead>;

        /// Reads exactly enough bytes to fill `into`.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if the buffer has insufficient data.
        fn read_exact(&mut self, into: &mut [u8]) -> Result<(), DidntRead>;

        /// Returns the number of bytes remaining to be read from this buffer.
        fn remaining(&self) -> usize;

        /// Reads exactly `len` bytes and passes them as `Chunk`s to the
        /// closure.
        ///
        /// For buffers composed of multiple slices, this may invoke the closure
        /// multiple times with different `Chunk` instances whose total
        /// length is exactly `len`.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if the buffer has insufficient data.
        fn read_chunks<F: FnMut(Chunk)>(&mut self, len: usize, for_each_slice: F) -> Result<(), DidntRead>;

        /// Reads exactly `len` bytes, returning them as a single `Chunk`.
        ///
        /// This enables zero-copy reading when the underlying buffer supports
        /// it.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if the buffer has insufficient data.
        fn read_chunk(&mut self, len: usize) -> Result<Chunk, DidntRead>;

        /// Reads a single byte from the buffer.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if the buffer is empty.
        fn read_u8(&mut self) -> Result<u8, DidntRead> {
            let mut byte = 0;
            let read = self.read(core::slice::from_mut(&mut byte))?;
            if read.get() == 1 { Ok(byte) } else { Err(DidntRead) }
        }

        /// Returns `true` if there is more data to read.
        fn can_read(&self) -> bool {
            self.remaining() != 0
        }
    }

    /// A trait for readers that support moving the read position forward or
    /// backward.
    pub(crate) trait AdvanceableReader: Reader {
        /// Skips forward by `offset` bytes without reading them.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if `offset` exceeds the remaining bytes.
        fn skip(&mut self, offset: usize) -> Result<(), DidntRead>;

        /// Moves backward by `offset` bytes, allowing previously read data to
        /// be read again.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if `offset` exceeds the amount of data already
        /// read.
        fn backtrack(&mut self, offset: usize) -> Result<(), DidntRead>;

        /// Advances the read position by a signed offset.
        ///
        /// Positive values skip forward, negative values backtrack.
        ///
        /// # Errors
        ///
        /// Returns `DidntRead` if the offset would move beyond the buffer's
        /// bounds.
        fn advance(&mut self, offset: isize) -> Result<(), DidntRead> {
            if offset > 0 {
                self.skip(offset as usize)
            } else {
                self.backtrack((-offset) as usize)
            }
        }
    }

    /// A trait for readers that support marking positions and seeking to them.
    ///
    /// This is similar to `AdvanceableReader` but uses explicit marks instead
    /// of offsets.
    pub(crate) trait SeekableReader: Reader {
        /// The type used to represent a marked position.
        type Mark;

        /// Marks the current position in the buffer.
        fn mark(&mut self) -> Self::Mark;

        /// Seeks to a previously marked position.
        ///
        /// Returns `true` if the seek was successful, `false` otherwise.
        fn seek(&mut self, mark: Self::Mark) -> bool;
    }

    /// Error type indicating a siphon operation failed.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct DidntSiphon;

    /// A trait for readers that can transfer data directly to a writer.
    ///
    /// This enables efficient data transfer between buffers without
    /// intermediate copying.
    pub(crate) trait SiphonableReader: Reader {
        /// Transfers as much data as possible from this reader to `writer`.
        ///
        /// Returns the number of bytes transferred.
        ///
        /// # Errors
        ///
        /// Returns `DidntSiphon` if no data could be transferred.
        #[allow(unused)]
        fn siphon<W>(&mut self, writer: &mut W) -> Result<NonZeroUsize, DidntSiphon>
        where
            W: super::writer::Writer;
    }

    /// A trait for types that can provide a reader.
    pub(crate) trait HasReader {
        /// The type of reader this type provides.
        type Reader: Reader;

        /// Returns the most appropriate reader for `self`.
        fn reader(self) -> Self::Reader;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BoxBuf, Bytes, Chunk,
        reader::{HasReader, Reader, SiphonableReader},
        writer::{HasWriter, Writer},
    };

    const BYTES: usize = 18;

    const WBS0: u8 = 0;
    const WBS1: u8 = 1;
    const WBS2: [u8; 4] = [2, 3, 4, 5];
    const WBS3: [u8; 4] = [6, 7, 8, 9];
    const WBS4: [u8; 4] = [10, 11, 12, 13];
    const WBS5: [u8; 4] = [14, 15, 16, 17];

    macro_rules! run_write {
        ($buffer:expr) => {{
            println!(">>> Write");
            let mut writer = $buffer.writer();
            assert!(writer.can_write());

            writer.write_u8(WBS0).unwrap();
            writer.write_u8(WBS1).unwrap();

            let w = writer.write(&WBS2).unwrap();
            assert_eq!(4, w.get());

            writer.write_exact(&WBS3).unwrap();
            writer.write_exact(&WBS4).unwrap();

            // SAFETY: callback returns the length of the buffer
            unsafe {
                writer.with_slot(4, |mut buffer| {
                    let w = buffer.write(&WBS5).unwrap();
                    assert_eq!(4, w.get());
                    w.get()
                })
            }
            .unwrap();
        }};
    }

    macro_rules! run_read {
        ($buffer:expr) => {
            println!(">>> Read");
            let mut reader = $buffer.reader();

            let b = reader.read_u8().unwrap();
            assert_eq!(WBS0, b);
            assert_eq!(BYTES - 1, reader.remaining());
            let b = reader.read_u8().unwrap();
            assert_eq!(WBS1, b);
            assert_eq!(BYTES - 2, reader.remaining());

            let mut rbs: [u8; 4] = [0, 0, 0, 0];
            let r = reader.read(&mut rbs).unwrap();
            assert_eq!(4, r.get());
            assert_eq!(BYTES - 6, reader.remaining());
            assert_eq!(WBS2, rbs);

            reader.read_exact(&mut rbs).unwrap();
            assert_eq!(BYTES - 10, reader.remaining());
            assert_eq!(WBS3, rbs);

            reader.read_exact(&mut rbs).unwrap();
            assert_eq!(BYTES - 14, reader.remaining());
            assert_eq!(WBS4, rbs);

            reader.read_exact(&mut rbs).unwrap();
            assert_eq!(BYTES - 18, reader.remaining());
            assert_eq!(WBS5, rbs);

            assert!(reader.read(&mut rbs).is_err());
            assert!(reader.read_u8().is_err());
            assert!(reader.read_exact(&mut rbs).is_err());
        };
    }

    macro_rules! run_empty {
        ($buffer:expr) => {
            let mut s = [0u8; 64];

            println!(">>> Read empty");
            let mut reader = $buffer.reader();
            assert!(reader.read_u8().is_err());
            assert!(reader.read(&mut s).is_err());
            assert!(reader.read_exact(&mut s).is_err());
        };
    }

    macro_rules! run_bound {
        ($buffer:expr, $capacity:expr) => {
            println!(">>> Write bound");
            let mut writer = $buffer.writer();

            for i in 0..$capacity {
                writer.write_u8(i as u8).unwrap();
            }
            assert!(writer.write_u8(0).is_err());

            println!(">>> Read bound");
            let mut reader = $buffer.reader();

            for i in 0..$capacity {
                let j = reader.read_u8().unwrap();
                assert_eq!(i as u8, j);
            }
            assert!(reader.read_u8().is_err());
        };
    }

    macro_rules! run_siphon {
        ($from:expr, $fcap:expr, $into:expr, $icap:expr) => {
            println!(">>> Write siphon");
            {
                let mut writer = $from.writer();
                for i in 0..$fcap {
                    writer.write_u8(i as u8).unwrap();
                }
            }

            println!(">>> Read siphon");
            let mut reader = $from.reader();

            let mut read = 0;
            while read < $fcap {
                $into.clear();
                let written = {
                    let mut writer = $into.writer();
                    reader.siphon(&mut writer).unwrap()
                };

                let mut reader = $into.reader();
                for i in read..read + written.get() {
                    let j = reader.read_u8().unwrap();
                    assert_eq!(i as u8, j);
                }
                assert!(reader.read_u8().is_err());

                read += written.get();
            }
            assert_eq!(read, $fcap);
        };
    }

    #[test]
    fn buffer_slice() {
        println!("Buffer Slice");
        let mut sbuf = [0u8; BYTES];
        run_write!(sbuf.as_mut());
        run_read!(sbuf.as_mut());
    }

    #[test]
    fn buffer_vec() {
        println!("Buffer Vec");
        let mut vbuf = vec![];
        run_empty!(vbuf);
        run_write!(&mut vbuf);
        run_read!(&vbuf);
    }

    #[test]
    fn buffer_bbuf() {
        println!("Buffer BBuf");
        let capacity = 1 + u8::MAX as usize;
        let mut bbuf = BoxBuf::with_capacity(capacity);
        run_empty!(bbuf);
        run_write!(bbuf);
        run_read!(bbuf);

        bbuf.clear();

        run_bound!(bbuf, capacity);
    }

    #[test]
    fn buffer_bytes() {
        println!("Buffer Bytes");
        let mut bytes = Bytes::new();
        run_empty!(bytes);
        run_write!(bytes);
        run_read!(bytes);

        let bytes = Bytes::from(vec![]);
        run_empty!(bytes);
    }

    #[test]
    fn buffer_chunk() {
        println!("Buffer Chunk");
        let mut vbuf = vec![];
        run_write!(&mut vbuf);

        let mut chunk = Chunk::from(vbuf);
        run_read!(chunk);

        let mut chunk = Chunk::from(vec![]);
        run_empty!(chunk);
    }

    #[test]
    fn buffer_siphon() {
        let capacity = 1 + u8::MAX as usize;

        println!("Buffer Siphon BBuf({capacity}) -> BBuf({capacity})");
        let mut bbuf1 = BoxBuf::with_capacity(capacity);
        let mut bbuf2 = BoxBuf::with_capacity(capacity);
        run_siphon!(bbuf1, capacity, bbuf2, capacity);

        println!("Buffer Siphon Bytes({capacity}) -> Bytes({capacity})");
        let mut bytes1 = Bytes::new();
        let mut bytes2 = Bytes::new();
        run_siphon!(bytes1, capacity, bytes2, capacity);

        println!("Buffer Siphon Bytes({capacity}) -> BBuf({capacity})");
        let mut bytes1 = Bytes::new();
        let mut bbuf1 = BoxBuf::with_capacity(capacity);
        run_siphon!(bytes1, capacity, bbuf1, capacity);

        let capacity2 = 1 + capacity / 2;
        println!("Buffer Siphon BBuf({capacity}) -> BBuf({capacity2})");
        let mut bbuf1 = BoxBuf::with_capacity(capacity);
        let mut bbuf2 = BoxBuf::with_capacity(capacity2);
        run_siphon!(bbuf1, capacity, bbuf2, capacity2);

        println!("Buffer Siphon Bytes({capacity}) -> BBuf({capacity2})");
        let mut bytes1 = Bytes::new();
        let mut bbuf1 = BoxBuf::with_capacity(capacity2);
        run_siphon!(bytes1, capacity, bbuf1, capacity2);
    }
}
