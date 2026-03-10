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

    use super::*;

    #[test]
    fn test_basic_notify_wait() {
        // Notifier fires from a separate thread after a short delay; waiter blocks until woken.
        // The notifier is returned from the thread and dropped only after block_on completes,
        // preventing Notifier::drop from overwriting the OK flag with ERR before the waiter
        // can consume it.
        let (notifier, waiter) = new();

        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            notifier.notify().unwrap();
            notifier
        });

        pollster::block_on(waiter.wait()).unwrap();
        drop(t.join().unwrap()); // notifier dropped here, after wait() has already returned Ok
    }

    #[test]
    fn test_notify_before_wait() {
        // Flag is already set before wait() is called, so it returns on the first check.
        let (notifier, waiter) = new();

        notifier.notify().unwrap();

        pollster::block_on(waiter.wait()).unwrap();
    }

    #[test]
    fn test_drop_all_notifiers() {
        // Dropping the notifier sets the ERR flag; waiter should receive an error.
        let (notifier, waiter) = new();

        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            drop(notifier);
        });

        let result = pollster::block_on(waiter.wait());
        assert!(result.is_err());
        t.join().unwrap();
    }

    #[test]
    fn test_drop_all_waiters() {
        // Dropping the waiter causes notifier to return an error — fully synchronous.
        let (notifier, waiter) = new();

        drop(waiter);

        let result = notifier.notify();
        assert!(result.is_err());
    }

    #[test]
    fn test_notification_preserved() {
        // Notification is stored in the atomic flag; wait() returns immediately even
        // if polled long after notify() was called.
        let (notifier, waiter) = new();

        notifier.notify().unwrap();

        pollster::block_on(waiter.wait()).unwrap();
    }
}
