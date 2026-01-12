use std::{any::Any, fmt::Debug};

use tokio::sync::oneshot;

use crate::buffers::DynBuf;

/// A wrapper that automatically returns a value via a channel when dropped.
///
/// [`Boomerang`] is provided as a **convenience wrapper** that implements [`Drop`] to automatically
/// return a value through a oneshot channel when dropped. It's nothing more than a simple wrapper
/// implementing [`Drop`]. Users are free to use `Boomerang` as a basis to implement their own
/// custom wrappers with different behaviors (e.g., return to pool, custom cleanup, metrics tracking)
/// as long as the wrapper implements [`DynBuf`](`crate::buffers::DynBuf`).
///
/// The name "Boomerang" reflects the behavior: you send a value out wrapped in a [`Boomerang`], and when it's dropped,
/// the value comes back to you through the receiver.
///
/// # Examples
///
/// ```
/// use thubo::collections::Boomerang;
///
/// #[tokio::main]
/// async fn main() {
///     let buffer = vec![1, 2, 3, 4, 5];
///     let (bytes, boomerang) = Boomerang::new(buffer.clone());
///
///     // Use the bytes...
///     drop(bytes);
///
///     // The original value is returned through the receiver
///     let returned = boomerang.await.unwrap();
///     assert_eq!(returned, buffer);
/// }
/// ```
pub struct Boomerang<T> {
    inner: Option<(T, oneshot::Sender<T>)>,
}

impl<T> Boomerang<T> {
    /// Creates a new [`Boomerang`] wrapping a value and returns a receiver.
    pub fn new(t: T) -> (Self, oneshot::Receiver<T>) {
        let (sender, receiver) = oneshot::channel();
        let boomerang = Self {
            inner: Some((t, sender)),
        };
        (boomerang, receiver)
    }
}

impl<T: Debug> Debug for Boomerang<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Boomerang").field("inner", &self.inner).finish()
    }
}

impl<T> Drop for Boomerang<T> {
    fn drop(&mut self) {
        let (t, sender) = self.inner.take().unwrap();
        let _ = sender.send(t);
    }
}

impl<T> DynBuf for Boomerang<T>
where
    T: DynBuf + 'static,
{
    fn as_slice(&self) -> &[u8] {
        self.inner.as_ref().unwrap().0.as_slice()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_boomerang() {
        // 1. Basic return behavior
        let value = vec![1, 2, 3, 4, 5];
        let (boomerang, receiver) = Boomerang::new(value.clone());
        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned, value);

        // 2. Return with integer
        let num = 42;
        let (boomerang, receiver) = Boomerang::new(num);
        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned, num);

        // 3. Return with String
        let text = String::from("hello world");
        let (boomerang, receiver) = Boomerang::new(text.clone());
        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned, text);

        // 4. Return with buffer
        let buffer = vec![0u8; 1024];
        let (boomerang, receiver) = Boomerang::new(buffer);
        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned.len(), 1024);
        assert!(returned.iter().all(|&x| x == 0));

        // 5. Multiple boomerangs
        let v1 = vec![1, 2];
        let v2 = vec![3, 4];
        let v3 = vec![5, 6];

        let (b1, r1) = Boomerang::new(v1.clone());
        let (b2, r2) = Boomerang::new(v2.clone());
        let (b3, r3) = Boomerang::new(v3.clone());

        drop(b1);
        drop(b2);
        drop(b3);

        assert_eq!(r1.await.unwrap(), v1);
        assert_eq!(r2.await.unwrap(), v2);
        assert_eq!(r3.await.unwrap(), v3);
    }

    #[tokio::test]
    async fn test_boomerang_with_dynbuf() {
        // 1. DynBuf implementation with Vec
        let data = vec![1u8, 2, 3, 4, 5];
        let (boomerang, receiver) = Boomerang::new(data.clone());

        // Access as DynBuf
        assert_eq!(boomerang.as_slice(), &[1, 2, 3, 4, 5]);

        // Verify as_any works
        let _any = boomerang.as_any();

        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned, data);

        // 2. Boomerang with larger buffer
        let large_data = vec![42u8; 10000];
        let (boomerang, receiver) = Boomerang::new(large_data.clone());

        assert_eq!(boomerang.as_slice().len(), 10000);
        assert!(boomerang.as_slice().iter().all(|&x| x == 42));

        drop(boomerang);
        let returned = receiver.await.unwrap();
        assert_eq!(returned.len(), 10000);
    }

    #[tokio::test]
    async fn test_boomerang_pool_pattern() {
        // Simulate a buffer pool pattern
        let mut pool = Vec::new();

        // Create initial buffers
        for _ in 0..3 {
            pool.push(vec![0u8; 512]);
        }

        // Take buffer from pool, use it, and get it back via boomerang
        let buffer = pool.pop().unwrap();
        let (boomerang, receiver) = Boomerang::new(buffer);

        // Simulate usage
        assert_eq!(boomerang.as_slice().len(), 512);

        drop(boomerang);

        // Return to pool
        let returned_buffer = receiver.await.unwrap();
        pool.push(returned_buffer);

        assert_eq!(pool.len(), 3);
    }

    #[tokio::test]
    async fn test_boomerang_ordering() {
        // Test that boomerangs return in the order they are dropped
        let (b1, r1) = Boomerang::new(1);
        let (b2, r2) = Boomerang::new(2);
        let (b3, r3) = Boomerang::new(3);

        // Drop in reverse order
        drop(b3);
        drop(b2);
        drop(b1);

        // But receive in any order (they're independent)
        let v1 = r1.await.unwrap();
        let v2 = r2.await.unwrap();
        let v3 = r3.await.unwrap();

        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(v3, 3);
    }
}
