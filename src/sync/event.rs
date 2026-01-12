use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use event_listener::{Event as EventLib, IntoNotification};

// Error types
const WAIT_ERR_STR: &str = "No notifier available";

/// Error returned when a wait operation fails because all notifiers have been dropped.
pub struct WaitError;

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl fmt::Debug for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(WAIT_ERR_STR)
    }
}

impl std::error::Error for WaitError {}

const NOTIFY_ERR_STR: &str = "No waiter available";

/// Error returned when a notify operation fails because all waiters have been dropped.
pub struct NotifyError;

impl fmt::Display for NotifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl fmt::Debug for NotifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(NOTIFY_ERR_STR)
    }
}

impl std::error::Error for NotifyError {}

/// Internal shared state for the event notification mechanism.
struct EventInner {
    /// The underlying event listener for async notifications.
    event: EventLib,
    /// Atomic flag tracking event state (unset, ok, or error).
    flag: AtomicU8,
}

/// Event flag is unset (no notification pending).
const UNSET: u8 = 0;
/// Event flag is set (notification available).
const OK: u8 = 1 << 0;
/// Event flag indicates error (all notifiers/waiters dropped).
const ERR: u8 = 1 << 1;

/// Result of checking the event flag state.
#[repr(u8)]
enum EventCheck {
    /// No notification is pending.
    Unset = UNSET,
    /// A notification is available.
    Ok = OK,
    /// The event is in error state.
    Err = ERR,
}

/// Result of setting the event flag.
#[repr(u8)]
enum EventSet {
    /// Flag was successfully set.
    Ok = OK,
    /// The event is in error state.
    Err = ERR,
}

impl EventInner {
    /// Checks and atomically clears the OK flag, returning the event state.
    fn check(&self) -> EventCheck {
        let f = self.flag.fetch_and(!OK, Ordering::AcqRel);
        if f & ERR != 0 {
            return EventCheck::Err;
        }
        if f == OK {
            return EventCheck::Ok;
        }
        EventCheck::Unset
    }

    /// Atomically sets the OK flag, returning whether the operation succeeded.
    fn set(&self) -> EventSet {
        let f = self.flag.fetch_or(OK, Ordering::AcqRel);
        if f & ERR != 0 {
            return EventSet::Err;
        }
        EventSet::Ok
    }

    /// Marks the event as errored (all notifiers or waiters dropped).
    fn err(&self) {
        self.flag.store(ERR, Ordering::Release);
        self.event.notify(1);
    }
}

pub(crate) fn new() -> (Notifier, Waiter) {
    let inner = Arc::new(EventInner {
        event: EventLib::new(),
        flag: AtomicU8::new(UNSET),
    });
    (Notifier(inner.clone()), Waiter(inner))
}

#[repr(transparent)]
pub(crate) struct Notifier(Arc<EventInner>);

impl Notifier {
    #[inline]
    pub(crate) fn notify(&self) -> Result<(), NotifyError> {
        // Set the flag.
        match self.0.set() {
            EventSet::Ok => {
                // Only call notify_additional if there are waiters listening
                self.0.event.notify(1.additional().relaxed());
                Ok(())
            }
            EventSet::Err => Err(NotifyError),
        }
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        self.0.err();
    }
}

#[repr(transparent)]
pub struct Waiter(Arc<EventInner>);

impl Waiter {
    #[inline]
    pub(crate) async fn wait(&self) -> Result<(), WaitError> {
        // Wait until the flag is set.
        loop {
            // Check the flag.
            match self.0.check() {
                EventCheck::Ok => return Ok(()),
                EventCheck::Unset => {}
                EventCheck::Err => return Err(WaitError),
            }

            // Actually wait for notification
            let listener = self.0.event.listen();

            match self.0.check() {
                EventCheck::Ok => return Ok(()),
                EventCheck::Unset => {}
                EventCheck::Err => return Err(WaitError),
            }

            listener.await;
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        // The last Waiter has been dropped, close the event
        self.0.err();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::timeout;

    use super::*;

    #[tokio::test]
    async fn test_basic_notify_wait() {
        // Basic notification and wait
        let (notifier, waiter) = new();

        let wait_task = tokio::spawn(async move {
            waiter.wait().await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(10)).await;
        notifier.notify().unwrap();

        wait_task.await.unwrap();
    }

    #[tokio::test]
    async fn test_notify_before_wait() {
        // Notification sent before wait - should return immediately
        let (notifier, waiter) = new();

        notifier.notify().unwrap();

        // Wait should return immediately
        let result = timeout(Duration::from_millis(100), waiter.wait()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }

    #[tokio::test]
    async fn test_drop_all_notifiers() {
        // Dropping all notifiers causes waiters to error
        let (notifier, waiter) = new();

        let wait_task = tokio::spawn(async move { waiter.wait().await });

        tokio::time::sleep(Duration::from_millis(10)).await;
        drop(notifier);

        let result = wait_task.await.unwrap();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_drop_all_waiters() {
        // Dropping all waiters causes notifiers to error
        let (notifier, waiter) = new();

        drop(waiter);

        let result = notifier.notify();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_notification_preserved() {
        // Notification is preserved if no waiter is waiting
        let (notifier, waiter) = new();

        notifier.notify().unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Wait should return immediately with the preserved notification
        let result = timeout(Duration::from_millis(100), waiter.wait()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }
}
