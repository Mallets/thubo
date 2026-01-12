use core::{cmp, num::NonZeroUsize};
use std::{
    io,
    ops::{Bound, RangeBounds},
};

use super::{
    Chunk, ChunkWriter,
    reader::{AdvanceableReader, DidntRead, DidntSiphon, HasReader, Reader, SiphonableReader},
    writer::{DidntWrite, HasWriter, Writer},
};
use crate::{buffers::reader::SeekableReader, collections::SingleOrVec};

/// A collection of non-contiguous byte chunks that supports efficient reading
/// and writing.
///
/// [`Bytes`] is a container for multiple [`Chunk`] instances, providing a
/// unified interface for working with non-contiguous byte sequences. It
/// supports both sequential and random access patterns via its [std::io::Read]
/// and [std::io::Seek] implementation.
///
/// The internal representation optimizes for the common case of a single chunk,
/// but can efficiently handle multiple chunks when needed.
///
/// # Examples
///
/// ## Basic Usage
///
/// ```
/// use thubo::Bytes;
///
/// // Create from a single chunk
/// let bytes: Bytes = vec![1, 2, 3, 4].into();
/// assert_eq!(bytes.len(), 4);
///
/// // Create empty and push chunks
/// let mut bytes = Bytes::new();
/// bytes.push(vec![1, 2].into());
/// bytes.push(vec![3, 4].into());
/// assert_eq!(bytes.len(), 4);
/// ```
///
/// ## Reading with `std::io::Read`
///
/// ```
/// use std::io::Read;
///
/// use thubo::Bytes;
///
/// let bytes: Bytes = vec![0, 1, 2, 3, 4, 5, 6, 7].into();
/// let mut reader = bytes.reader();
///
/// // Read into a buffer
/// let mut buf = [0u8; 4];
/// reader.read_exact(&mut buf).unwrap();
/// assert_eq!(buf, [0, 1, 2, 3]);
///
/// // Continue reading
/// reader.read_exact(&mut buf).unwrap();
/// assert_eq!(buf, [4, 5, 6, 7]);
/// ```
///
/// ## Seeking with `std::io::Seek`
///
/// ```
/// use std::io::{Read, Seek, SeekFrom};
///
/// use thubo::Bytes;
///
/// let bytes: Bytes = vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9].into();
/// let mut reader = bytes.reader();
///
/// // Seek to a specific position
/// reader.seek(SeekFrom::Start(5)).unwrap();
/// let mut buf = [0u8; 2];
/// reader.read_exact(&mut buf).unwrap();
/// assert_eq!(buf, [5, 6]);
///
/// // Seek relative to current position
/// reader.seek(SeekFrom::Current(-3)).unwrap();
/// reader.read_exact(&mut buf).unwrap();
/// assert_eq!(buf, [4, 5]);
///
/// // Seek from end
/// reader.seek(SeekFrom::End(-2)).unwrap();
/// reader.read_exact(&mut buf).unwrap();
/// assert_eq!(buf, [8, 9]);
/// ```
///
/// ## Writing with `std::io::Write`
///
/// ```
/// use std::io::Write;
///
/// use thubo::Bytes;
///
/// let mut bytes = Bytes::new();
/// {
///     let mut writer = bytes.writer();
///     writer.write_all(b"Hello, ").unwrap();
///     writer.write_all(b"World!").unwrap();
/// } // Writer is dropped, flushing data to bytes
///
/// assert_eq!(bytes.to_vec(), b"Hello, World!");
/// ```
#[derive(Debug, Clone, Default, Eq)]
pub struct Bytes {
    chunks: SingleOrVec<Chunk>,
}

impl Bytes {
    /// Creates a new empty [`Bytes`] instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            chunks: SingleOrVec::new(),
        }
    }

    /// Creates a new [`Bytes`] instance containing a single chunk.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::{Bytes, Chunk};
    ///
    /// let chunk: Chunk = vec![1, 2, 3].into();
    /// let bytes = Bytes::single(chunk);
    /// assert_eq!(bytes.len(), 3);
    /// ```
    #[must_use]
    pub const fn single(chunk: Chunk) -> Self {
        Self {
            chunks: SingleOrVec::single(chunk),
        }
    }

    /// Returns the total number of bytes across all chunks.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2, 3].into());
    /// bytes.push(vec![4, 5].into());
    /// assert_eq!(bytes.len(), 5);
    /// ```
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.chunks.as_ref().iter().fold(0, |len, slice| len + slice.len())
    }

    /// Returns `true` if the [`Bytes`] contains no data.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Removes all chunks from the [`Bytes`], leaving it empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes: Bytes = vec![1, 2, 3].into();
    /// bytes.clear();
    /// assert!(bytes.is_empty());
    /// ```
    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    /// Appends a chunk to the end of the [`Bytes`].
    ///
    /// NOTE: Empty chunks are automatically ignored and not added.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2].into());
    /// bytes.push(vec![3, 4].into());
    /// assert_eq!(bytes.len(), 4);
    /// ```
    pub fn push(&mut self, chunk: Chunk) {
        if !chunk.is_empty() {
            self.chunks.push(chunk);
        }
    }

    /// Returns an iterator over references to the chunks.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2].into());
    /// bytes.push(vec![3, 4].into());
    ///
    /// let chunk_count = bytes.chunks().count();
    /// assert_eq!(chunk_count, 2);
    /// ```
    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> + '_ {
        self.chunks.as_ref().iter()
    }

    /// Converts the [`Bytes`] into a single `Chunk`.
    ///
    /// If the [`Bytes`] is empty, returns an empty chunk. If it contains a
    /// single chunk, returns a clone of that chunk. Otherwise, copies all
    /// data into a new contiguous [`Vec<u8>`] and converts it to a
    /// [`Chunk`].
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2].into());
    /// bytes.push(vec![3, 4].into());
    ///
    /// let chunk = bytes.to_chunk();
    /// assert_eq!(&*chunk, &[1, 2, 3, 4]);
    /// ```
    pub fn to_chunk(&self) -> Chunk {
        match self.chunks.as_slice() {
            [] => [].into(),
            [chunk] => chunk.clone(),
            _ => self.to_vec().into(),
        }
    }

    /// Returns an iterator over byte slices from all chunks.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2].into());
    /// bytes.push(vec![3, 4].into());
    ///
    /// let slices: Vec<_> = bytes.slices().collect();
    /// assert_eq!(slices.len(), 2);
    /// ```
    pub fn slices(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.chunks().map(Chunk::as_slice)
    }

    /// Copies all bytes into a contiguous [`Vec<u8>`].
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2].into());
    /// bytes.push(vec![3, 4].into());
    ///
    /// let vec = bytes.to_vec();
    /// assert_eq!(vec, vec![1, 2, 3, 4]);
    /// ```
    pub fn to_vec(&self) -> Vec<u8> {
        self.slices().fold(Vec::with_capacity(self.len()), |mut acc, s| {
            acc.extend_from_slice(s);
            acc
        })
    }

    /// Creates a view into a subrange of the bytes.
    ///
    /// Returns a new [`Bytes`] instance that references the specified range of
    /// bytes without copying. The range is specified in terms of byte
    /// positions across all chunks.
    ///
    /// Returns [`None`] if the range is out of bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Bytes;
    ///
    /// let bytes: Bytes = vec![0, 1, 2, 3, 4, 5].into();
    ///
    /// let view = bytes.view(2..5).unwrap();
    /// assert_eq!(view.to_vec(), vec![2, 3, 4]);
    ///
    /// assert!(bytes.view(10..).is_none());
    /// ```
    pub fn view(&self, range: impl RangeBounds<usize>) -> Option<Self> {
        let start_delta = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };
        let end_delta = match range.end_bound() {
            Bound::Included(&n) => n + 1,
            Bound::Excluded(&n) => n,
            Bound::Unbounded => self.len(),
        };

        let mut reader = self.reader();
        reader.skip(start_delta).ok()?;

        let mut bytes = Self::new();
        let len = end_delta - start_delta;
        reader.read_chunks(len, |c| bytes.chunks.push(c)).ok()?;

        Some(bytes)
    }

    /// Creates a reader for sequentially reading bytes from this [`Bytes`]
    /// instance.
    ///
    /// The returned [`BytesReader`] implements [`std::io::Read`], and
    /// [`std::io::Seek`], providing flexible access to the byte data. The
    /// reader starts at position 0 and can traverse all chunks seamlessly.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Read;
    ///
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// bytes.push(vec![1, 2, 3].into());
    /// bytes.push(vec![4, 5, 6].into());
    ///
    /// let mut reader = bytes.reader();
    /// let mut buf = [0u8; 2];
    /// reader.read_exact(&mut buf).unwrap();
    /// assert_eq!(buf, [1, 2]);
    /// ```
    pub fn reader(&self) -> BytesReader<'_> {
        BytesReader {
            inner: self,
            cursor: BytesPos { slice: 0, byte: 0 },
        }
    }

    /// Creates a writer for sequentially writing bytes to this [`Bytes`]
    /// instance.
    ///
    /// The returned [`BytesWriter`] implements [`std::io::Write`], allowing
    /// standard I/O operations. Written data is automatically added as
    /// chunks to the [`Bytes`] when the writer is dropped or when chunk
    /// boundaries are crossed.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    ///
    /// use thubo::Bytes;
    ///
    /// let mut bytes = Bytes::new();
    /// {
    ///     let mut writer = bytes.writer();
    ///     writer.write_all(b"Hello, ").unwrap();
    ///     writer.write_all(b"World!").unwrap();
    /// } // Writer is dropped here, flushing data to bytes
    ///
    /// assert_eq!(bytes.to_vec(), b"Hello, World!");
    /// ```
    pub fn writer(&mut self) -> BytesWriter<'_> {
        BytesWriter {
            inner: self,
            writer: ChunkWriter::new(),
        }
    }
}

impl PartialEq for Bytes {
    fn eq(&self, other: &Self) -> bool {
        let mut self_slices = self.slices();
        let mut other_slices = other.slices();
        let mut current_self = self_slices.next();
        let mut current_other = other_slices.next();
        loop {
            match (current_self, current_other) {
                (None, None) => return true,
                (None, _) | (_, None) => return false,
                (Some(l), Some(r)) => {
                    let cmp_len = l.len().min(r.len());
                    // SAFETY: cmp_len is the minimum length between l and r slices.
                    let lhs = super::unsafe_slice!(l, ..cmp_len);
                    let rhs = super::unsafe_slice!(r, ..cmp_len);
                    if lhs != rhs {
                        return false;
                    }
                    if cmp_len == l.len() {
                        current_self = self_slices.next();
                    } else {
                        // SAFETY: cmp_len is the minimum length between l and r slices.
                        let lhs = super::unsafe_slice!(l, cmp_len..);
                        current_self = Some(lhs);
                    }
                    if cmp_len == r.len() {
                        current_other = other_slices.next();
                    } else {
                        // SAFETY: cmp_len is the minimum length between l and r slices.
                        let rhs = super::unsafe_slice!(r, cmp_len..);
                        current_other = Some(rhs);
                    }
                }
            }
        }
    }
}

// From impls
impl<T> From<T> for Bytes
where
    T: Into<Chunk>,
{
    fn from(t: T) -> Self {
        Bytes::single(t.into())
    }
}

// Reader (Sealed)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BytesPos {
    pub(crate) slice: usize,
    pub(crate) byte: usize,
}

impl BytesPos {
    pub(crate) const fn zero() -> Self {
        Self { slice: 0, byte: 0 }
    }
}

/// A reader for sequentially reading bytes from a [`Bytes`] instance.
///
/// `BytesReader` provides efficient sequential access to byte data stored
/// across multiple chunks. It implements [`std::io::Read`] and
/// [`std::io::Seek`].
///
/// The reader maintains an internal cursor that tracks the current read
/// position across all chunks, automatically advancing through chunk boundaries
/// as data is read.
///
/// # Examples
///
/// ```
/// use std::io::Read;
///
/// use thubo::Bytes;
///
/// let bytes: Bytes = vec![1, 2, 3, 4, 5].into();
/// let mut reader = bytes.reader();
///
/// let mut buf = [0u8; 3];
/// reader.read(&mut buf).unwrap();
/// assert_eq!(buf, [1, 2, 3]);
/// ```
#[derive(Debug, Clone)]
pub struct BytesReader<'a> {
    inner: &'a Bytes,
    cursor: BytesPos,
}

impl SeekableReader for BytesReader<'_> {
    type Mark = BytesPos;

    fn mark(&mut self) -> BytesPos {
        self.cursor
    }

    fn seek(&mut self, mark: Self::Mark) -> bool {
        match self.inner.chunks.get(mark.slice) {
            Some(slice) if mark.byte <= slice.len() => {
                self.cursor = mark;
                true
            }
            _ => false,
        }
    }
}

impl<'a> HasReader for &'a Bytes {
    type Reader = BytesReader<'a>;

    fn reader(self) -> Self::Reader {
        self.reader()
    }
}

impl Reader for BytesReader<'_> {
    fn read(&mut self, mut into: &mut [u8]) -> Result<NonZeroUsize, DidntRead> {
        let mut read = 0;
        while let Some(slice) = self.inner.chunks.get(self.cursor.slice) {
            // Subslice from the current read slice
            // SAFETY: validity of self.cursor.byte is ensured by the read logic.
            let from = super::unsafe_slice!(slice.as_slice(), self.cursor.byte..);
            // Take the minimum length among read and write slices
            let len = from.len().min(into.len());
            // Copy the slice content
            // SAFETY: len is the minimum length between from and into slices.
            let lhs = super::unsafe_slice_mut!(into, ..len);
            let rhs = super::unsafe_slice!(from, ..len);
            lhs.copy_from_slice(rhs);
            // Advance the write slice
            // SAFETY: len is the minimum length between from and into slices.
            into = super::unsafe_slice_mut!(into, len..);
            // Update the counter
            read += len;
            // Move the byte cursor
            self.cursor.byte += len;
            // We consumed all the current read slice, move to the next slice
            if self.cursor.byte == slice.len() {
                self.cursor.slice += 1;
                self.cursor.byte = 0;
            }
            // We have read everything we had to read
            if into.is_empty() {
                break;
            }
        }
        NonZeroUsize::new(read).ok_or(DidntRead)
    }

    fn read_exact(&mut self, into: &mut [u8]) -> Result<(), DidntRead> {
        let len = Reader::read(self, into)?;
        if len.get() == into.len() {
            Ok(())
        } else {
            Err(DidntRead)
        }
    }

    fn read_u8(&mut self) -> Result<u8, DidntRead> {
        let slice = self.inner.chunks.get(self.cursor.slice).ok_or(DidntRead)?;

        let byte = *slice.get(self.cursor.byte).ok_or(DidntRead)?;
        self.cursor.byte += 1;
        if self.cursor.byte == slice.len() {
            self.cursor.slice += 1;
            self.cursor.byte = 0;
        }
        Ok(byte)
    }

    fn remaining(&self) -> usize {
        // SAFETY: self.cursor.slice validity is ensured by the reader
        let s = super::unsafe_slice!(self.inner.chunks.as_ref(), self.cursor.slice..);
        s.iter().fold(0, |acc, it| acc + it.len()) - self.cursor.byte
    }

    fn read_chunks<F: FnMut(Chunk)>(&mut self, len: usize, mut f: F) -> Result<(), DidntRead> {
        if self.remaining() < len {
            return Err(DidntRead);
        }

        let iter = BytesSliceIterator {
            reader: self,
            remaining: len,
        };
        for slice in iter {
            f(slice);
        }

        Ok(())
    }

    fn read_chunk(&mut self, len: usize) -> Result<Chunk, DidntRead> {
        let slice = self.inner.chunks.get(self.cursor.slice).ok_or(DidntRead)?;
        match (slice.len() - self.cursor.byte).cmp(&len) {
            cmp::Ordering::Less => {
                let mut buffer = vec![0u8; len];
                Reader::read_exact(self, &mut buffer)?;
                Ok(buffer.into())
            }
            cmp::Ordering::Equal => {
                let s = slice.view(self.cursor.byte..).ok_or(DidntRead)?;
                self.cursor.slice += 1;
                self.cursor.byte = 0;
                Ok(s)
            }
            cmp::Ordering::Greater => {
                let start = self.cursor.byte;
                self.cursor.byte += len;
                slice.view(start..self.cursor.byte).ok_or(DidntRead)
            }
        }
    }
}

impl SiphonableReader for BytesReader<'_> {
    fn siphon<W>(&mut self, writer: &mut W) -> Result<NonZeroUsize, DidntSiphon>
    where
        W: Writer,
    {
        let mut read = 0;
        while let Some(slice) = self.inner.chunks.get(self.cursor.slice) {
            // Subslice from the current read slice
            // SAFETY: self.cursor.byte is ensured by the reader.
            let from = super::unsafe_slice!(slice.as_slice(), self.cursor.byte..);
            // Copy the slice content
            match writer.write(from) {
                Ok(len) => {
                    // Update the counter
                    read += len.get();
                    // Move the byte cursor
                    self.cursor.byte += len.get();
                    // We consumed all the current read slice, move to the next slice
                    if self.cursor.byte == slice.len() {
                        self.cursor.slice += 1;
                        self.cursor.byte = 0;
                    }
                }
                Err(_) => {
                    return NonZeroUsize::new(read).ok_or(DidntSiphon);
                }
            }
        }
        NonZeroUsize::new(read).ok_or(DidntSiphon)
    }
}

impl io::Read for BytesReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match <Self as Reader>::read(self, buf) {
            Ok(n) => Ok(n.get()),
            Err(_) => Ok(0),
        }
    }
}

impl AdvanceableReader for BytesReader<'_> {
    fn skip(&mut self, offset: usize) -> Result<(), DidntRead> {
        let mut remaining_offset = offset;
        while remaining_offset > 0 {
            let s = self.inner.chunks.get(self.cursor.slice).ok_or(DidntRead)?;
            let remains_in_current_slice = s.len() - self.cursor.byte;
            let advance = remaining_offset.min(remains_in_current_slice);
            remaining_offset -= advance;
            self.cursor.byte += advance;
            if self.cursor.byte == s.len() {
                self.cursor.slice += 1;
                self.cursor.byte = 0;
            }
        }
        Ok(())
    }

    fn backtrack(&mut self, offset: usize) -> Result<(), DidntRead> {
        let mut remaining_offset = offset;
        while remaining_offset > 0 {
            let backtrack = remaining_offset.min(self.cursor.byte);
            remaining_offset -= backtrack;
            self.cursor.byte -= backtrack;
            if self.cursor.byte == 0 {
                if self.cursor.slice == 0 {
                    break;
                }
                self.cursor.slice -= 1;
                self.cursor.byte = self.inner.chunks.get(self.cursor.slice).ok_or(DidntRead)?.len();
            }
        }
        if remaining_offset == 0 { Ok(()) } else { Err(DidntRead) }
    }
}

impl io::Seek for BytesReader<'_> {
    fn seek(&mut self, pos: io::SeekFrom) -> io::Result<u64> {
        let current_pos = self
            .inner
            .slices()
            .take(self.cursor.slice)
            .fold(0, |acc, s| acc + s.len())
            + self.cursor.byte;
        let current_pos = i64::try_from(current_pos).map_err(|e| io::Error::other(e.to_string()))?;

        let offset = match pos {
            io::SeekFrom::Start(s) => i64::try_from(s).unwrap_or(i64::MAX) - current_pos,
            io::SeekFrom::Current(s) => s,
            io::SeekFrom::End(s) => self.inner.len() as i64 + s - current_pos,
        };
        match self.advance(offset as isize) {
            Ok(()) => Ok((offset + current_pos) as u64),
            Err(_) => Err(io::Error::new(io::ErrorKind::InvalidInput, "InvalidInput")),
        }
    }
}

// Chunk iterator
pub(crate) struct BytesSliceIterator<'a, 'b> {
    reader: &'a mut BytesReader<'b>,
    remaining: usize,
}

impl Iterator for BytesSliceIterator<'_, '_> {
    type Item = Chunk;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        // SAFETY: self.reader.cursor.slice is ensured by the reader.
        let slice = super::unsafe_slice!(self.reader.inner.chunks.as_ref(), self.reader.cursor.slice);
        let start = self.reader.cursor.byte;
        // SAFETY: self.reader.cursor.byte is ensured by the reader.
        let current = super::unsafe_slice!(slice, start..);
        let len = current.len();
        match self.remaining.cmp(&len) {
            cmp::Ordering::Less => {
                let end = start + self.remaining;
                let slice = slice.view(start..end);
                self.reader.cursor.byte = end;
                self.remaining = 0;
                slice
            }
            cmp::Ordering::Equal => {
                let end = start + self.remaining;
                let slice = slice.view(start..end);
                self.reader.cursor.slice += 1;
                self.reader.cursor.byte = 0;
                self.remaining = 0;
                slice
            }
            cmp::Ordering::Greater => {
                let end = start + len;
                let slice = slice.view(start..end);
                self.reader.cursor.slice += 1;
                self.reader.cursor.byte = 0;
                self.remaining -= len;
                slice
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (1, None)
    }
}

/// A writer for sequentially writing bytes to a [`Bytes`] instance.
///
/// `BytesWriter` provides efficient sequential write access, implementing
/// [`std::io::Write`]. Data written to the writer is buffered internally and
/// added as chunks to the parent [`Bytes`] when the writer is dropped or when
/// explicit chunk boundaries are created.
///
/// The writer automatically handles the conversion of written data into
/// appropriately sized chunks for optimal performance.
///
/// # Examples
///
/// ```
/// use std::io::Write;
///
/// use thubo::Bytes;
///
/// let mut bytes = Bytes::new();
/// {
///     let mut writer = bytes.writer();
///     writer.write_all(b"test data").unwrap();
/// } // Data is flushed to bytes when writer is dropped
///
/// assert_eq!(bytes.to_vec(), b"test data");
/// ```
#[derive(Debug)]
pub struct BytesWriter<'a> {
    inner: &'a mut Bytes,
    writer: ChunkWriter,
}

impl Drop for BytesWriter<'_> {
    fn drop(&mut self) {
        let written = self.writer.snapshot();
        if !written.is_empty() {
            self.inner.push(written);
        }
    }
}

impl<'a> HasWriter for &'a mut Bytes {
    type Writer = BytesWriter<'a>;

    fn writer(self) -> Self::Writer {
        self.writer()
    }
}

impl Writer for BytesWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite> {
        self.writer.write(bytes)
    }

    fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite> {
        self.writer.write_exact(bytes)
    }

    fn remaining(&self) -> usize {
        usize::MAX
    }

    fn write_chunk(&mut self, slice: &Chunk) -> Result<(), DidntWrite> {
        let written = self.writer.snapshot();
        // SAFETY: `self.inner` is valid as guaranteed by `self.writer` borrow,
        // and `self.writer` has been overwritten
        if !written.is_empty() {
            self.inner.push(written);
        }
        if !slice.is_empty() {
            self.inner.push(slice.clone());
        }
        Ok(())
    }

    unsafe fn with_slot<F>(&mut self, len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        unsafe { self.writer.with_slot(len, write) }
    }
}

impl io::Write for BytesWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match <Self as Writer>::write(self, buf) {
            Ok(n) => Ok(n.get()),
            Err(_) => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Bytes {
    #[cfg(test)]
    pub(crate) fn rand(len: usize) -> Self {
        let mut bytes = Bytes::new();
        bytes.push(Chunk::rand(len));
        bytes
    }
}

impl AsRef<Bytes> for Bytes {
    fn as_ref(&self) -> &Bytes {
        self
    }
}

impl<T> Extend<T> for Bytes
where
    T: Into<Chunk>,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for elem in iter {
            self.push(elem.into());
        }
    }
}

impl IntoIterator for Bytes {
    type Item = Chunk;
    type IntoIter = crate::collections::single_or_vec::IntoIter<Chunk>;

    fn into_iter(self) -> Self::IntoIter {
        self.chunks.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::buffers::{Bytes, Chunk, reader::Reader};

    #[test]
    fn bytes_eq() {
        let slice: Chunk = vec![0u8, 1, 2, 3, 4, 5, 6, 7].into();

        let mut bytes1 = Bytes::new();
        bytes1.push(slice.view(..4).unwrap());
        bytes1.push(slice.view(4..8).unwrap());

        let mut bytes2 = Bytes::new();
        bytes2.push(slice.view(..1).unwrap());
        bytes2.push(slice.view(1..4).unwrap());
        bytes2.push(slice.view(4..8).unwrap());

        assert_eq!(bytes1, bytes2);

        let mut bytes1 = Bytes::new();
        bytes1.push(slice.view(2..4).unwrap());
        bytes1.push(slice.view(4..8).unwrap());

        let mut bytes2 = Bytes::new();
        bytes2.push(slice.view(2..3).unwrap());
        bytes2.push(slice.view(3..6).unwrap());
        bytes2.push(slice.view(6..8).unwrap());

        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn bytes_seek() {
        use std::io::Seek;

        let mut buf = Bytes::new();
        buf.push([0u8, 1u8, 2u8, 3u8].into());
        buf.push([4u8, 5u8, 6u8, 7u8, 8u8].into());
        buf.push([9u8, 10u8, 11u8, 12u8, 13u8, 14u8].into());
        let mut reader = buf.reader();

        assert_eq!(reader.stream_position().unwrap(), 0);
        assert_eq!(reader.read_u8().unwrap(), 0);
        assert_eq!(reader.seek(std::io::SeekFrom::Current(6)).unwrap(), 7);
        assert_eq!(reader.read_u8().unwrap(), 7);
        assert_eq!(reader.seek(std::io::SeekFrom::Current(-5)).unwrap(), 3);
        assert_eq!(reader.read_u8().unwrap(), 3);
        assert_eq!(reader.seek(std::io::SeekFrom::Current(10)).unwrap(), 14);
        assert_eq!(reader.read_u8().unwrap(), 14);
        reader.seek(std::io::SeekFrom::Current(100)).unwrap_err();

        assert_eq!(reader.seek(std::io::SeekFrom::Start(0)).unwrap(), 0);
        assert_eq!(reader.read_u8().unwrap(), 0);
        assert_eq!(reader.seek(std::io::SeekFrom::Start(12)).unwrap(), 12);
        assert_eq!(reader.read_u8().unwrap(), 12);
        assert_eq!(reader.seek(std::io::SeekFrom::Start(15)).unwrap(), 15);
        reader.read_u8().unwrap_err();
        reader.seek(std::io::SeekFrom::Start(100)).unwrap_err();

        assert_eq!(reader.seek(std::io::SeekFrom::End(0)).unwrap(), 15);
        reader.read_u8().unwrap_err();
        assert_eq!(reader.seek(std::io::SeekFrom::End(-5)).unwrap(), 10);
        assert_eq!(reader.read_u8().unwrap(), 10);
        assert_eq!(reader.seek(std::io::SeekFrom::End(-15)).unwrap(), 0);
        assert_eq!(reader.read_u8().unwrap(), 0);
        reader.seek(std::io::SeekFrom::End(-20)).unwrap_err();

        assert_eq!(reader.seek(std::io::SeekFrom::Start(10)).unwrap(), 10);
        reader.seek(std::io::SeekFrom::Current(-100)).unwrap_err();
    }

    #[test]
    fn bytes_view() {
        let bytes: Bytes = [0, 1, 2, 3, 4, 5, 6, 7].into();

        // 1. Full range view
        let view1: Bytes = bytes.view(..).unwrap();
        assert_eq!(view1.to_vec(), vec![0, 1, 2, 3, 4, 5, 6, 7]);

        // 2. View to exact length
        let view2 = bytes.view(0..8).unwrap();
        assert_eq!(view2.to_vec(), vec![0, 1, 2, 3, 4, 5, 6, 7]);

        // 3. View from index 1 to end
        let view3 = bytes.view(1..).unwrap();
        assert_eq!(view3.to_vec(), vec![1, 2, 3, 4, 5, 6, 7]);

        // 4. View with inclusive range
        let view4 = bytes.view(2..=4).unwrap();
        assert_eq!(view4.to_vec(), vec![2, 3, 4]);

        // 5. View of single element
        let view5 = bytes.view(3..4).unwrap();
        assert_eq!(view5.to_vec(), vec![3]);

        // 6. View from start
        let view6 = bytes.view(..3).unwrap();
        assert_eq!(view6.to_vec(), vec![0, 1, 2]);

        // 7. Empty view
        let view7 = bytes.view(4..4).unwrap();
        assert_eq!(view7.to_vec(), vec![]);

        // 8. View at end boundary
        let view8 = bytes.view(7..=7).unwrap();
        assert_eq!(view8.to_vec(), vec![7]);

        // 9. View of last two elements
        let view9 = bytes.view(6..).unwrap();
        assert_eq!(view9.to_vec(), vec![6, 7]);

        // 10. Nested views - view of a view
        let view10 = view3.view(1..4).unwrap();
        assert_eq!(view10.to_vec(), vec![2, 3, 4]);

        // 11. Out of bounds checks
        assert!(bytes.view(100..).is_none());
        assert!(bytes.view(..100).is_none());
    }
}
