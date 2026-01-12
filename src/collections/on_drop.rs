use std::{any::Any, fmt::Debug};

use crate::buffers::DynBuf;

/// A wrapper that executes a custom cleanup function when dropped.
///
/// [`OnDrop`] is provided as a **convenience wrapper** that implements [`Drop`] to automatically
/// execute a custom cleanup function when dropped. It's nothing more than a simple wrapper
/// implementing [`Drop`]. Users are free to use `OnDrop` as a basis to implement their own
/// custom wrappers with different behaviors (e.g., return to pool, custom cleanup, metrics tracking)
/// as long as the wrapper implements [`DynBuf`](`crate::buffers::DynBuf`).
///
/// # Examples
///
/// ```
/// use std::sync::{
///     Arc, Mutex,
///     atomic::{AtomicUsize, Ordering},
/// };
///
/// use thubo::collections::OnDrop;
///
/// static COUNTER: AtomicUsize = AtomicUsize::new(0);
///
/// {
///     let value = vec![1, 2, 3];
///     let _guard = OnDrop::new(value, move |v| {
///         COUNTER.fetch_add(1, Ordering::SeqCst);
///     });
///     // `guard` is dropped here
/// }
///
/// assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
/// ```
pub struct OnDrop<T, D>
where
    D: FnOnce(T) + Send + Sync,
{
    inner: Option<(T, D)>,
}

impl<T, D> OnDrop<T, D>
where
    D: FnOnce(T) + Send + Sync,
{
    /// Creates a new [`OnDrop`] wrapper with a value and cleanup function.
    pub fn new(t: T, drop: D) -> Self {
        Self { inner: Some((t, drop)) }
    }
}

impl<T: Debug, D> Debug for OnDrop<T, D>
where
    D: FnOnce(T) + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnDrop")
            .field("inner", &self.inner.as_ref().unwrap().0)
            .finish()
    }
}

impl<T, D> DynBuf for OnDrop<T, D>
where
    T: DynBuf + 'static,
    D: FnOnce(T) + Send + Sync + 'static,
{
    fn as_slice(&self) -> &[u8] {
        self.inner.as_ref().unwrap().0.as_slice()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T, D> Drop for OnDrop<T, D>
where
    D: FnOnce(T) + Send + Sync,
{
    fn drop(&mut self) {
        let (t, d) = self.inner.take().unwrap();
        (d)(t)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::channel,
    };

    use super::*;

    #[test]
    fn test_ondrop() {
        // Simple counter
        static COUNTER: AtomicUsize = AtomicUsize::new(0);

        let value = 42;
        let v = OnDrop::new(value, move |v| {
            COUNTER.store(v, Ordering::SeqCst);
        });
        assert_eq!(COUNTER.load(Ordering::SeqCst), 0);

        drop(v);
        assert_eq!(COUNTER.load(Ordering::SeqCst), value);

        // Vec
        let (sender, receiver) = channel();
        let data = vec![1u8, 2, 3, 4, 5];
        let guard = OnDrop::new(data.clone(), move |v| {
            sender.send(v).unwrap();
        });

        drop(guard);
        assert_eq!(data, receiver.try_recv().unwrap())
    }
}
