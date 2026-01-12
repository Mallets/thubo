use core::{
    fmt,
    num::NonZeroUsize,
    ops::{Bound, Deref, RangeBounds},
};
use std::{any::Any, sync::Arc};

use super::{
    reader::{DidntRead, HasReader, Reader},
    writer::{DidntWrite, Writer},
};
use crate::buffers::writer::HasWriter;

/************************************ */
/* DYN BUFFER */
/************************************ */
/// A trait for types that can be stored in an [`Chunk`] and provide a byte
/// slice view.
///
/// This trait allows different buffer types (like [`Vec<u8>`], [`String`], and
/// fixed-size arrays) to be used as the backing storage for an [`Chunk`]. The
/// trait requires `Send + Sync` to ensure thread-safe sharing via [`Arc`].
///
/// # Examples
///
/// ```
/// use std::any::Any;
///
/// use thubo::DynBuf;
///
/// struct FortyTwo;
///
/// impl DynBuf for FortyTwo {
///     fn as_slice(&self) -> &[u8] {
///         &[42]
///     }
///
///     fn as_any(&self) -> &dyn Any {
///         self // Returns the concrete type, not the trait object
///     }
/// }
/// ```
pub trait DynBuf: Send + Sync {
    /// Returns a byte slice view of the entire buffer.
    fn as_slice(&self) -> &[u8];

    /// Returns a reference to the concrete type as [`std::any::Any`] for
    /// downcasting.
    ///
    /// This method is essential for [`Chunk::downcast_ref`] to work correctly.
    /// Each implementation must return `self` directly (i.e. the concrete
    /// type). Without this method, it would be impossible to downcast from
    /// [`Arc<dyn DynBuf>`] back to the original concrete type like
    /// [`Vec<u8>`].
    ///
    /// Implementors should always return `self` directly.
    fn as_any(&self) -> &dyn Any;
}

impl DynBuf for Vec<u8> {
    fn as_slice(&self) -> &[u8] {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DynBuf for Box<[u8]> {
    fn as_slice(&self) -> &[u8] {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<const N: usize> DynBuf for [u8; N] {
    fn as_slice(&self) -> &[u8] {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DynBuf for &'static [u8] {
    fn as_slice(&self) -> &[u8] {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DynBuf for String {
    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl DynBuf for &'static str {
    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/************************************ */
/* CHUNK */
/************************************ */
/// A cloneable wrapper to a contiguous slice of bytes.
///
/// [`Chunk`] provides a cheaply cloneable, reference-counted view into a byte
/// buffer. It uses `Arc` internally to share the underlying buffer across
/// multiple instances, making it ideal for scenarios where you need to pass
/// buffer data without copying.
///
/// The buffer maintains its own start and end offsets, allowing for efficient
/// subslicing via the [`view`](Chunk::view) method without copying data.
///
/// # Examples
///
/// ## Basic Usage
///
/// ```
/// use thubo::Chunk;
///
/// // Create from Vec<u8>
/// let data = vec![1, 2, 3, 4, 5];
/// let chunk: Chunk = data.into();
/// assert_eq!(chunk.len(), 5);
/// assert_eq!(chunk.as_slice(), &[1, 2, 3, 4, 5]);
///
/// // Create from static array
/// let arr_chunk: Chunk = [10, 20, 30].into();
/// assert_eq!(arr_chunk.as_slice(), &[10, 20, 30]);
///
/// // Create from String
/// let string_chunk: Chunk = "hello".to_string().into();
/// assert_eq!(string_chunk.as_slice(), b"hello");
/// ```
///
/// ## Creating Views
///
/// ```
/// use thubo::Chunk;
///
/// let chunk: Chunk = vec![0, 1, 2, 3, 4, 5, 6, 7].into();
///
/// // Create a view of part of the buffer
/// let view = chunk.view(2..5).unwrap();
/// assert_eq!(view.as_slice(), &[2, 3, 4]);
///
/// // Views can be chained
/// let sub_view = view.view(1..3).unwrap();
/// assert_eq!(sub_view.as_slice(), &[3, 4]);
///
/// // Use different range types
/// let from_start = chunk.view(..3).unwrap();
/// assert_eq!(from_start.as_slice(), &[0, 1, 2]);
///
/// let to_end = chunk.view(5..).unwrap();
/// assert_eq!(to_end.as_slice(), &[5, 6, 7]);
/// ```
///
/// ## Cheap Cloning
///
/// ```
/// use thubo::Chunk;
///
/// let chunk: Chunk = vec![1, 2, 3, 4].into();
///
/// // Cloning is cheap - only increments Arc reference count
/// let clone1 = chunk.clone();
/// let clone2 = chunk.clone();
///
/// // All share the same underlying buffer
/// assert_eq!(chunk, clone1);
/// assert_eq!(clone1, clone2);
/// ```
///
/// ## Downcasting to Concrete Types
///
/// ```
/// use thubo::Chunk;
///
/// let original = vec![1, 2, 3, 4];
/// let chunk: Chunk = original.clone().into();
///
/// // Downcast back to the original type
/// let vec_ref: &Vec<u8> = chunk.downcast_ref().unwrap();
/// assert_eq!(vec_ref, &original);
///
/// // Downcasting to wrong type returns None
/// let box_ref: Option<&Box<[u8]>> = chunk.downcast_ref();
/// assert!(box_ref.is_none());
/// ```
#[derive(Clone)]
pub struct Chunk {
    buf: Arc<dyn DynBuf>,
    start: usize,
    end: usize,
}

impl Chunk {
    /// Creates a new [`Chunk`] with the specified start and end offsets.
    ///
    /// # Errors
    ///
    /// Returns the original buffer if `start > end` or `end >
    /// buf.as_slice().len()`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use thubo::{Chunk, DynBuf};
    ///
    /// let data = vec![1, 2, 3, 4, 5];
    /// let buf = Chunk::new(Arc::new(data), 1, 4).unwrap_or_else(|_| panic!("Out of bound"));
    /// assert_eq!(buf.len(), 3);
    /// ```
    pub fn new(buf: Arc<dyn DynBuf>, start: usize, end: usize) -> Result<Chunk, Arc<dyn DynBuf>> {
        if start <= end && end <= buf.as_slice().len() {
            Ok(Self { buf, start, end })
        } else {
            Err(buf)
        }
    }

    /// Creates a new [`Chunk`] without validating the bounds.
    ///
    /// This function skips the bounds checking performed by [`Chunk::new`],
    /// allowing direct construction of a [`Chunk`] with arbitrary start and end
    /// indices.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - `start <= end`
    /// - `end <= buf.as_slice().len()`
    ///
    /// Violating these invariants will result in undefined behavior when the
    /// [`Chunk`] is used, as methods like [`Chunk::as_slice`] may read
    /// out-of-bounds memory or panic.
    #[must_use]
    pub unsafe fn new_unchecked(buf: Arc<dyn DynBuf>, start: usize, end: usize) -> Chunk {
        Self { buf, start, end }
    }

    /// Returns the length of the buffer in bytes.
    pub const fn len(&self) -> usize {
        self.end - self.start
    }

    /// Returns `true` if the buffer has a length of 0.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a byte slice view of the buffer.
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: The slice indices self.start..self.end are guaranteed to be valid
        // because:
        // 1. When constructed via `Self::new()`, bounds are validated: start <= end <= buf.len()
        // 2. When constructed via `Self::view()`, the new indices are validated relative to the current view's range,
        //    maintaining: 0 <= start <= end <= self.len()
        // 3. `Self::new_unchecked()` requires the caller to uphold these invariants via unsafe contract
        // Therefore, get_unchecked is safe as the range is always within the buffer's
        // bounds.
        unsafe { self.buf.as_slice().get_unchecked(self.start..self.end) }
    }

    /// Attempts to downcast the underlying buffer to a concrete type reference.
    ///
    /// This method allows access to the original buffer type that was used to
    /// create the [`Chunk`]. Since [`Chunk`] stores buffers as [`Arc<dyn
    /// DynBuf>`], this provides a way to recover the concrete type if
    /// needed.
    ///
    /// # Returns
    ///
    /// Returns `Some(&T)` if the underlying buffer is of type `T`, or `None` if
    /// it's a different type.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Chunk;
    ///
    /// // Create a Chunk from a Vec
    /// let data = vec![1u8, 2, 3, 4];
    /// let chunk: Chunk = data.into();
    ///
    /// // Successfully downcast to Vec<u8>
    /// let vec_ref: &Vec<u8> = chunk.downcast_ref().unwrap();
    /// assert_eq!(vec_ref, &vec![1u8, 2, 3, 4]);
    ///
    /// // Fails to downcast to a different type
    /// let box_ref: Option<&Box<[u8]>> = chunk.downcast_ref();
    /// assert!(box_ref.is_none());
    /// ```
    #[must_use]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.buf.as_any().downcast_ref()
    }

    /// Creates a view into a subrange of this buffer.
    ///
    /// This method creates a new [`Chunk`] that shares the same underlying
    /// buffer but with adjusted offsets. The range is relative to the
    /// current buffer's view.
    ///
    /// # Returns
    ///
    /// Returns `Some(Chunk)` if the range is valid, or `None` if the range is
    /// out of bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// use thubo::Chunk;
    ///
    /// let buf: Chunk = vec![1, 2, 3, 4, 5].into();
    /// let view = buf.view(1..4).unwrap();
    /// assert_eq!(&*view, &[2, 3, 4]);
    ///
    /// // Can create views of views
    /// let sub_view = view.view(1..2).unwrap();
    /// assert_eq!(&*sub_view, &[3]);
    /// ```
    #[must_use]
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
        (start_delta <= end_delta && end_delta <= self.len()).then_some(Chunk {
            buf: Arc::clone(&self.buf),
            start: self.start + start_delta,
            end: self.start + end_delta,
        })
    }
}

impl Deref for Chunk {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl AsRef<[u8]> for Chunk {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl<Rhs: AsRef<[u8]> + ?Sized> PartialEq<Rhs> for Chunk {
    fn eq(&self, other: &Rhs) -> bool {
        self.as_slice() == other.as_ref()
    }
}

impl Eq for Chunk {}

impl fmt::Display for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:02x?}", self.as_slice())
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:02x?}", self.as_slice())
    }
}

// From impls
impl<T> From<Arc<T>> for Chunk
where
    T: DynBuf + 'static,
{
    fn from(buf: Arc<T>) -> Self {
        let end = buf.as_slice().len();
        Self { buf, start: 0, end }
    }
}

impl<T> From<T> for Chunk
where
    T: DynBuf + 'static,
{
    fn from(buf: T) -> Self {
        Self::from(Arc::new(buf))
    }
}

/// Internal writer for creating [`Chunk`] instances.
///
/// This writer allows appending data and creating snapshots of the written data
/// as [`Chunk`] instances. Each snapshot captures the data written since the
/// last snapshot.
#[derive(Debug)]
pub(crate) struct ChunkWriter {
    inner: Arc<Vec<u8>>,
    start: usize,
}

impl ChunkWriter {
    /// Creates a new empty `ChunkWriter`.
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Vec::new()),
            start: 0,
        }
    }

    /// Creates an [`Chunk`] snapshot of the data written since the last
    /// snapshot.
    ///
    /// The snapshot captures all bytes from the last snapshot position to the
    /// current write position. After creating a snapshot, the writer's
    /// internal position is updated so subsequent snapshots will only
    /// include newly written data.
    pub(crate) fn snapshot(&mut self) -> Chunk {
        let dynbuf = Chunk {
            buf: self.inner.clone(),
            start: self.start,
            end: self.inner.len(),
        };
        self.start = self.inner.len();
        dynbuf
    }

    fn writer(&mut self) -> &mut Vec<u8> {
        // SAFETY: This Arc-to-mutable-reference cast is sound because of the following
        // invariants:
        // 1. Exclusive access: We have `&mut self`, guaranteeing no other code can access `self.inner`
        // 2. Single ownership: The Arc was created in `new()` and never cloned to external references. When
        //    `snapshot()` clones the Arc, it's stored in a `Chunk` that's immediately returned, but crucially, the
        //    returned Chunk can never have mutable access back to this writer
        // 3. Temporal guarantee: Between snapshots, we retain the only mutable pathway to the Arc's data. Snapshots
        //    create immutable views into past data ranges [start..end], while we continue writing beyond that range
        // 4. No aliasing: The mutable reference we return has a lifetime bound to `&mut self`, preventing it from
        //    escaping while `self` might be accessed elsewhere
        //
        // This pattern is safe because while multiple Arcs may point to the Vec (via
        // snapshots), only this ChunkWriter can mutate it, and only while
        // holding exclusive mutable access.
        unsafe { &mut *(Arc::as_ptr(&self.inner) as *mut Vec<u8>) }.writer()
    }
}

impl Writer for ChunkWriter {
    fn write(&mut self, bytes: &[u8]) -> Result<NonZeroUsize, DidntWrite> {
        let mut writer = self.writer();
        let len = writer.write(bytes)?;
        Ok(len)
    }

    fn write_exact(&mut self, bytes: &[u8]) -> Result<(), DidntWrite> {
        self.write(bytes).map(|_| ())
    }

    fn remaining(&self) -> usize {
        usize::MAX
    }

    unsafe fn with_slot<F>(&mut self, len: usize, write: F) -> Result<NonZeroUsize, DidntWrite>
    where
        F: FnOnce(&mut [u8]) -> usize,
    {
        // SAFETY: same precondition as the enclosing function
        let len = unsafe { self.writer().with_slot(len, write) }?;
        Ok(len)
    }
}

// Reader
impl HasReader for &mut Chunk {
    type Reader = Self;

    fn reader(self) -> Self::Reader {
        self
    }
}

impl Reader for &mut Chunk {
    fn read(&mut self, into: &mut [u8]) -> Result<NonZeroUsize, DidntRead> {
        let mut reader = self.as_slice().reader();
        let len = reader.read(into)?;
        // we trust `Reader` impl for `&[u8]` to not overflow the size of the slice
        self.start += len.get();
        Ok(len)
    }

    fn read_exact(&mut self, into: &mut [u8]) -> Result<(), DidntRead> {
        let mut reader = self.as_slice().reader();
        reader.read_exact(into)?;
        // we trust `Reader` impl for `&[u8]` to not overflow the size of the slice
        self.start += into.len();
        Ok(())
    }

    fn read_u8(&mut self) -> Result<u8, DidntRead> {
        let mut reader = self.as_slice().reader();
        let res = reader.read_u8()?;
        // we trust `Reader` impl for `&[u8]` to not overflow the size of the slice
        self.start += 1;
        Ok(res)
    }

    fn read_chunks<F: FnMut(Chunk)>(&mut self, len: usize, mut f: F) -> Result<(), DidntRead> {
        let dynbuf = self.read_chunk(len)?;
        f(dynbuf);
        Ok(())
    }

    fn read_chunk(&mut self, len: usize) -> Result<Chunk, DidntRead> {
        let res = self.view(..len).ok_or(DidntRead)?;
        self.start += len;
        Ok(res)
    }

    fn remaining(&self) -> usize {
        self.len()
    }
}

impl Chunk {
    #[cfg(test)]
    pub(crate) fn rand(len: usize) -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        (0..len).map(|_| rng.random()).collect::<Vec<u8>>().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash() {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{Hash, Hasher},
        };

        let buf = vec![1, 2, 3, 4, 5];
        let mut buf_hasher = DefaultHasher::new();
        buf.hash(&mut buf_hasher);
        let buf_hash = buf_hasher.finish();

        let dynbuf: Chunk = buf.clone().into();
        let mut dynbuf_hasher = DefaultHasher::new();
        dynbuf.hash(&mut dynbuf_hasher);
        let dynbuf_hash = dynbuf_hasher.finish();

        assert_eq!(buf_hash, dynbuf_hash);
    }

    #[test]
    fn chunk_downcast() {
        // 1. Vec downcast
        let vec = vec![1u8, 2, 3, 4, 5];
        let chunk1: Chunk = vec.clone().into();
        let vec_ref = chunk1.downcast_ref::<Vec<u8>>().unwrap();
        assert_eq!(vec_ref, &vec);
        assert!(chunk1.downcast_ref::<Box<[u8]>>().is_none());
        assert!(chunk1.downcast_ref::<[u8; 5]>().is_none());

        // 2. Box downcast
        let boxed: Box<[u8]> = vec![1, 2, 3, 4, 5].into_boxed_slice();
        let chunk2: Chunk = boxed.clone().into();
        let box_ref = chunk2.downcast_ref::<Box<[u8]>>().unwrap();
        assert_eq!(box_ref.as_ref(), boxed.as_ref());
        assert!(chunk2.downcast_ref::<Vec<u8>>().is_none());

        // 3. Array downcast
        let array: [u8; 5] = [1, 2, 3, 4, 5];
        let chunk3: Chunk = array.into();
        let array_ref = chunk3.downcast_ref::<[u8; 5]>().unwrap();
        assert_eq!(array_ref, &array);
        assert!(chunk3.downcast_ref::<Vec<u8>>().is_none());
        assert!(chunk3.downcast_ref::<[u8; 4]>().is_none());
    }

    #[test]
    fn chunk_as_slice() {
        // 1. Full chunk
        let chunk1: Chunk = vec![1u8, 2, 3, 4, 5].into();
        let slice1 = chunk1.as_slice();
        assert_eq!(slice1, &[1, 2, 3, 4, 5]);
        assert_eq!(slice1.len(), 5);

        // 2. From view
        let chunk2: Chunk = vec![1u8, 2, 3, 4, 5, 6, 7, 8].into();
        let view2 = chunk2.view(2..6).unwrap();
        let slice2 = view2.as_slice();
        assert_eq!(slice2, &[3, 4, 5, 6]);
        assert_eq!(slice2.len(), 4);

        // 3. Empty view
        let chunk3: Chunk = vec![1u8, 2, 3, 4, 5].into();
        let view3 = chunk3.view(2..2).unwrap();
        let slice3 = view3.as_slice();
        assert_eq!(slice3, &[]);
        assert_eq!(slice3.len(), 0);
        assert!(slice3.is_empty());

        // 4. Single element
        let chunk4: Chunk = vec![42u8].into();
        let slice4 = chunk4.as_slice();
        assert_eq!(slice4, &[42]);
        assert_eq!(slice4.len(), 1);

        // 5. Different buffer types
        let vec_chunk: Chunk = vec![1u8, 2, 3].into();
        assert_eq!(vec_chunk.as_slice(), &[1, 2, 3]);

        let box_chunk: Chunk = vec![4u8, 5, 6].into_boxed_slice().into();
        assert_eq!(box_chunk.as_slice(), &[4, 5, 6]);

        let array_chunk: Chunk = [7u8, 8, 9].into();
        assert_eq!(array_chunk.as_slice(), &[7, 8, 9]);
    }

    #[test]
    fn chunk_view() {
        let data = vec![0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9];
        let chunk: Chunk = data.into();

        // 1. Full range
        let view1 = chunk.view(..).unwrap();
        assert_eq!(view1.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        // 2. Partial range
        let view2 = chunk.view(2..6).unwrap();
        assert_eq!(view2.as_slice(), &[2, 3, 4, 5]);

        // 3. From start
        let view3 = chunk.view(..3).unwrap();
        assert_eq!(view3.as_slice(), &[0, 1, 2]);

        // 4. To end
        let view4 = chunk.view(7..).unwrap();
        assert_eq!(view4.as_slice(), &[7, 8, 9]);

        // 5. Inclusive range
        let view5 = chunk.view(1..=4).unwrap();
        assert_eq!(view5.as_slice(), &[1, 2, 3, 4]);

        // 6. Single element
        let view6 = chunk.view(5..6).unwrap();
        assert_eq!(view6.as_slice(), &[5]);

        // 7. Empty range
        let view7 = chunk.view(3..3).unwrap();
        assert_eq!(view7.as_slice(), &[]);

        // 8. At boundaries
        let view8a = chunk.view(0..2).unwrap();
        assert_eq!(view8a.as_slice(), &[0, 1]);
        let view8b = chunk.view(8..10).unwrap();
        assert_eq!(view8b.as_slice(), &[8, 9]);
        let view8c = chunk.view(0..10).unwrap();
        assert_eq!(view8c.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        // 9. Nested views
        let view9a = chunk.view(2..8).unwrap();
        assert_eq!(view9a.as_slice(), &[2, 3, 4, 5, 6, 7]);
        let view9b = view9a.view(1..4).unwrap();
        assert_eq!(view9b.as_slice(), &[3, 4, 5]);
        let view9c = view9b.view(1..2).unwrap();
        assert_eq!(view9c.as_slice(), &[4]);

        // 10. Out of bounds
        assert!(chunk.view(0..20).is_none());
        assert!(chunk.view(5..20).is_none());
        assert!(chunk.view(15..).is_none());
        assert!(chunk.view(100..200).is_none());

        // 11. Sub-views with bounds checking
        let view11 = chunk.view(2..8).unwrap();
        assert_eq!(view11.as_slice(), &[2, 3, 4, 5, 6, 7]);
        let view11b = view11.view(1..4).unwrap();
        assert_eq!(view11b.as_slice(), &[3, 4, 5]);
        assert!(view11.view(0..10).is_none());
        assert!(view11.view(7..).is_none());

        // 12. Equality
        let view12a = chunk.view(2..5).unwrap();
        let view12b = chunk.view(2..5).unwrap();
        assert_eq!(view12a, view12b);
        let view12c = chunk.view(3..5).unwrap();
        assert_ne!(view12a, view12c);
    }
}
