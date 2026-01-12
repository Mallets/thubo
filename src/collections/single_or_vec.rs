//! A memory-efficient collection that optimizes for the common case of storing
//! a single element, but can grow to hold multiple elements when needed.
use core::{
    fmt, iter,
    ops::{Index, IndexMut},
    ptr, slice,
};
use std::ops::{Bound, RangeBounds};

/// A collection optimized for storing either a single element or multiple
/// elements.
///
/// This type is useful when you typically have one element, but occasionally
/// need more. It avoids heap allocation when storing a single element, reducing
/// memory overhead and improving cache locality.
///
/// # Memory Layout
///
/// - Empty: Uses an empty `Vec` (24 bytes on 64-bit systems)
/// - Single element: Stores the element inline (discriminant + element size)
/// - Multiple elements: Uses a `Vec` to store all elements
#[derive(Clone, Eq)]
pub(crate) enum SingleOrVec<T> {
    Single(T),
    Vec(Vec<T>),
}

impl<T> SingleOrVec<T> {
    /// Creates an empty collection.
    pub(crate) const fn new() -> Self {
        Self::Vec(Vec::new())
    }

    /// Creates a collection with a single element.
    pub(crate) const fn single(t: T) -> Self {
        Self::Single(t)
    }

    /// Adds an element to the collection.
    ///
    /// If the collection is empty, the element is stored inline without heap
    /// allocation. If the collection has one element, it transitions to a
    /// `Vec` and allocates on the heap.
    pub(crate) fn push(&mut self, value: T) {
        match self {
            Self::Vec(vec) if vec.capacity() == 0 => *self = Self::Single(value),
            Self::Single(first) => unsafe {
                let first = ptr::read(first);
                ptr::write(self, Self::Vec(vec![first, value]));
            },
            Self::Vec(vec) => vec.push(value),
        }
    }

    /// Shortens the collection, keeping the first `len` elements.
    ///
    /// If `len` is 0, the collection becomes empty. If `len` is greater than
    /// the current length, this has no effect.
    ///
    /// Note that this method has no effect on the allocated capacity of the
    /// vector.
    pub(crate) fn truncate(&mut self, len: usize) {
        if let Self::Vec(v) = self {
            v.truncate(len);
        } else if len == 0 {
            *self = Self::Vec(Vec::new());
        }
    }

    /// Removes all elements from the collection.
    pub(crate) fn clear(&mut self) {
        self.truncate(0);
    }

    /// Returns the number of elements in the collection.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Single(_) => 1,
            Self::Vec(v) => v.len(),
        }
    }

    /// Returns `true` if the collection contains no elements.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a slice view of the collection.
    pub(crate) fn as_slice(&self) -> &[T] {
        match self {
            Self::Single(v) => slice::from_ref(v),
            Self::Vec(v) => v.as_slice(),
        }
    }

    /// Returns a mutable slice view of the collection.
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        match self {
            Self::Single(v) => slice::from_mut(v),
            Self::Vec(v) => v.as_mut_slice(),
        }
    }

    /// Returns a reference to the element at the given index, or `None` if out
    /// of bounds.
    pub(crate) fn get(&self, index: usize) -> Option<&T> {
        self.as_slice().get(index)
    }

    /// Returns a reference to the element at the given index, or `None` if out
    /// of bounds.
    #[allow(unused)]
    pub(crate) fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.as_mut_slice().get_mut(index)
    }

    /// Removes and returns elements from the specified range as an iterator.
    ///
    /// This method consumes `self` and returns an iterator over the elements
    /// in the specified range. Elements outside the range are dropped.
    #[allow(unused)]
    pub(crate) fn drain(self, range: impl RangeBounds<usize>) -> impl Iterator<Item = T> {
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
        self.into_iter().skip(start_delta).take(end_delta - start_delta)
    }
}

impl<T> PartialEq for SingleOrVec<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}

impl<T> Default for SingleOrVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> AsRef<[T]> for SingleOrVec<T> {
    fn as_ref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> AsMut<[T]> for SingleOrVec<T> {
    fn as_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T> fmt::Debug for SingleOrVec<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_ref())
    }
}

impl<T> IntoIterator for SingleOrVec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Single(first) => IntoIter {
                last: Some(first),
                drain: Vec::new().into_iter(),
            },
            Self::Vec(v) => {
                let mut it = v.into_iter();
                IntoIter {
                    last: it.next_back(),
                    drain: it,
                }
            }
        }
    }
}

impl<T> iter::Extend<T> for SingleOrVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.push(value);
        }
    }
}

pub struct IntoIter<T> {
    pub drain: std::vec::IntoIter<T>,
    pub last: Option<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<Self::Item> {
        self.drain.next().or_else(|| self.last.take())
    }
}

impl<T> Index<usize> for SingleOrVec<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.as_slice()[index]
    }
}

impl<T> IndexMut<usize> for SingleOrVec<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.as_mut_slice()[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let items = SingleOrVec::<i32>::new();
        assert_eq!(items.len(), 0);
        assert!(items.is_empty());
        assert_eq!(items.as_ref(), &[]);
    }

    #[test]
    fn test_single() {
        let items = SingleOrVec::single(42);
        assert_eq!(items.len(), 1);
        assert!(!items.is_empty());
        assert_eq!(items.as_ref(), &[42]);
        assert_eq!(items[0], 42);
    }

    #[test]
    fn test_push_to_empty() {
        let mut items = SingleOrVec::new();
        items.push(1);
        assert_eq!(items.len(), 1);
        assert_eq!(items.as_ref(), &[1]);
    }

    #[test]
    fn test_push_to_single() {
        let mut items = SingleOrVec::single(1);
        items.push(2);
        assert_eq!(items.len(), 2);
        assert_eq!(items.as_ref(), &[1, 2]);
    }

    #[test]
    fn test_push_multiple() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        items.push(3);
        items.push(4);
        assert_eq!(items.len(), 4);
        assert_eq!(items.as_ref(), &[1, 2, 3, 4]);
    }

    #[test]
    fn test_get() {
        // 1. Get from Vec
        let mut items1 = SingleOrVec::new();
        items1.extend([10, 20, 30]);
        assert_eq!(items1.get(0), Some(&10));
        assert_eq!(items1.get(1), Some(&20));
        assert_eq!(items1.get(2), Some(&30));
        assert_eq!(items1.get(3), None);

        // 2. Get from single element
        let items2 = SingleOrVec::single(42);
        assert_eq!(items2.get(0), Some(&42));
        assert_eq!(items2.get(1), None);

        // 3. Get from empty
        let items3 = SingleOrVec::<i32>::new();
        assert_eq!(items3.get(0), None);

        // 4. get_mut from single element
        let mut items4 = SingleOrVec::single(42);
        if let Some(val) = items4.get_mut(0) {
            *val = 100;
        }
        assert_eq!(items4[0], 100);
        assert!(items4.get_mut(1).is_none());

        // 5. get_mut from Vec
        let mut items5 = SingleOrVec::new();
        items5.extend([1, 2, 3]);
        if let Some(val) = items5.get_mut(0) {
            *val = 10;
        }
        if let Some(val) = items5.get_mut(2) {
            *val = 30;
        }
        assert_eq!(items5.as_ref(), &[10, 2, 30]);
        assert!(items5.get_mut(3).is_none());
        assert!(items5.get_mut(100).is_none());

        // 6. get_mut from empty
        let mut items6 = SingleOrVec::<i32>::new();
        assert!(items6.get_mut(0).is_none());
    }

    #[test]
    fn test_index() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        assert_eq!(items[0], 1);
        assert_eq!(items[1], 2);
    }

    #[test]
    fn test_index_mut() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        items[0] = 10;
        items[1] = 20;
        assert_eq!(items.as_ref(), &[10, 20]);
    }

    #[test]
    fn test_truncate() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        items.push(3);
        items.truncate(2);
        assert_eq!(items.as_ref(), &[1, 2]);
        items.truncate(0);
        assert_eq!(items.as_ref(), &[]);
    }

    #[test]
    fn test_truncate_single() {
        let mut items = SingleOrVec::single(42);
        items.truncate(1);
        assert_eq!(items.as_ref(), &[42]);
        items.truncate(0);
        assert_eq!(items.as_ref(), &[]);
    }

    #[test]
    fn test_clear() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        items.clear();
        assert_eq!(items.len(), 0);
        assert!(items.is_empty());
    }

    #[test]
    fn test_as_mut() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        let slice = items.as_mut();
        slice[0] = 10;
        assert_eq!(items[0], 10);
    }

    #[test]
    fn test_into_iter() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        items.push(3);
        let collected: Vec<_> = items.into_iter().collect();
        assert_eq!(collected, vec![1, 2, 3]);
    }

    #[test]
    fn test_into_iter_single() {
        let items = SingleOrVec::single(42);
        let collected: Vec<_> = items.into_iter().collect();
        assert_eq!(collected, vec![42]);
    }

    #[test]
    fn test_into_iter_empty() {
        let items = SingleOrVec::<i32>::new();
        let collected: Vec<_> = items.into_iter().collect();
        assert_eq!(collected, vec![]);
    }

    #[test]
    fn test_extend() {
        let mut items = SingleOrVec::new();
        items.extend([1, 2, 3]);
        assert_eq!(items.as_ref(), &[1, 2, 3]);
        items.extend([4, 5]);
        assert_eq!(items.as_ref(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_clone() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        let cloned = items.clone();
        assert_eq!(cloned.as_ref(), items.as_ref());
    }

    #[test]
    fn test_eq() {
        let mut items1 = SingleOrVec::new();
        items1.push(1);
        items1.push(2);

        let mut items2 = SingleOrVec::new();
        items2.push(1);
        items2.push(2);

        assert_eq!(items1, items2);
    }

    #[test]
    fn test_debug() {
        let mut items = SingleOrVec::new();
        items.push(1);
        items.push(2);
        let debug_str = format!("{:?}", items);
        assert_eq!(debug_str, "[1, 2]");
    }

    #[test]
    fn test_default() {
        let items = SingleOrVec::<i32>::default();
        assert_eq!(items.len(), 0);
        assert!(items.is_empty());
    }

    #[test]
    fn test_drain() {
        // 1. Full range
        let mut items1 = SingleOrVec::new();
        items1.extend([1, 2, 3, 4]);
        let drained1: Vec<_> = items1.drain(..).collect();
        assert_eq!(drained1, vec![1, 2, 3, 4]);

        // 2. Partial range
        let mut items2 = SingleOrVec::new();
        items2.extend([1, 2, 3, 4]);
        let drained2: Vec<_> = items2.drain(1..3).collect();
        assert_eq!(drained2, vec![2, 3]);

        // 3. From start
        let mut items3 = SingleOrVec::new();
        items3.extend([1, 2, 3]);
        let drained3: Vec<_> = items3.drain(..2).collect();
        assert_eq!(drained3, vec![1, 2]);

        // 4. To end
        let mut items4 = SingleOrVec::new();
        items4.extend([1, 2, 3, 4]);
        let drained4: Vec<_> = items4.drain(2..).collect();
        assert_eq!(drained4, vec![3, 4]);

        // 5. Inclusive range
        let mut items5 = SingleOrVec::new();
        items5.extend([1, 2, 3, 4]);
        let drained5: Vec<_> = items5.drain(1..=2).collect();
        assert_eq!(drained5, vec![2, 3]);

        // 6. Single element - full drain
        let items6 = SingleOrVec::single(42);
        let drained6: Vec<_> = items6.drain(..).collect();
        assert_eq!(drained6, vec![42]);

        // 7. Single element - partial
        let items7 = SingleOrVec::single(42);
        let drained7: Vec<_> = items7.drain(0..1).collect();
        assert_eq!(drained7, vec![42]);

        // 8. Single element - empty range
        let items8 = SingleOrVec::single(42);
        let drained8: Vec<_> = items8.drain(1..).collect();
        assert_eq!(drained8, vec![]);

        // 9. Empty collection
        let items9 = SingleOrVec::<i32>::new();
        let drained9: Vec<_> = items9.drain(..).collect();
        assert_eq!(drained9, vec![]);

        // 10. Empty range
        let mut items10 = SingleOrVec::new();
        items10.extend([1, 2, 3]);
        let drained10: Vec<_> = items10.drain(2..2).collect();
        assert_eq!(drained10, vec![]);

        // 11. Single index
        let mut items11 = SingleOrVec::new();
        items11.extend([1, 2, 3, 4]);
        let drained11: Vec<_> = items11.drain(2..3).collect();
        assert_eq!(drained11, vec![3]);
    }
}
